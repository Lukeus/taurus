//! Commands that outlive the call that started them.
//!
//! [`crate::builtin::shell::RunCommand`] waits for what it starts, which is
//! right for almost everything: the model asked a question and the answer is
//! the output. It is wrong for the three things a long task actually needs — a
//! build from cold, a full test run, a server to develop against — because the
//! wait is capped at ten minutes and a turn spent waiting is a turn spending
//! nothing else.
//!
//! So a background command is started, left alone, and read later. What that
//! costs is the two guarantees the foreground path gets for free.
//!
//! # Its output
//!
//! Nobody is holding the pipes open on the model's behalf, so the output is
//! drained into a buffer as it arrives. The buffer is [`MAX_PENDING_BYTES`],
//! and past that the oldest is dropped. Both streams go into the one buffer in
//! the order they arrived: a background command is watched rather than parsed,
//! and that is the order a terminal would have shown.
//!
//! What a reader gets out of it is decided by a cursor rather than by emptying
//! it. The buffer counts every byte that ever went in, so a place in the
//! stream is a single number: [`Jobs::check`] keeps the model's, and the
//! window keeps its own. That is the difference between two readers and two
//! readers taking lines from each other — this used to drain on read, and a
//! pane drawing a build would have emptied the buffer the next
//! `check_command` was going to read, losing output the model would never
//! learn had existed. A cursor older than the buffer is told how many bytes
//! are gone between it and the oldest still held, which is what the drop count
//! used to say and now says it per reader.
//!
//! # What it changed
//!
//! The sweep that makes a command rewindable reads the workspace before it
//! runs and again when it finishes ([`crate::sweep`]). Here those are minutes
//! apart and in different turns, so the job carries its own pre-image from the
//! moment it started and spends it when the process exits. [`Jobs::reap`] is
//! called after every tool call, so the changes land in the turn that was
//! running when the command finished — not the one that started it, which by
//! then is history. A command still running when a turn ends is in no turn's
//! changed-file list yet, which is the honest answer: it has not finished
//! changing them.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::checkpoint::TurnRecorder;
use crate::sweep::Sweep;

/// How many commands may be running in the background at once.
///
/// Not a resource limit — eight `cargo build`s would be one machine's problem
/// either way. It is a limit on how much a model can lose track of: every one
/// of these is a process the user did not start and cannot see, and a roster
/// that needs scrolling is one nobody stops.
pub const MAX_JOBS: usize = 8;

/// How much of one command's output is kept.
///
/// The whole record the window has: a job's tab is drawn from this and from
/// nothing else, so what falls off the front is gone from the pane as well as
/// from the next check. A shell's scrollback lives in its own emulator and can
/// afford to be long; this is held in the host for every job at once, so the
/// worst case is this times [`MAX_JOBS`].
const MAX_PENDING_BYTES: usize = 256 * 1024;

/// The most one `check_command` will hand back.
///
/// A different number from [`MAX_PENDING_BYTES`] because the readers are
/// different. What the window shows costs a scrollbar; what the model reads
/// costs context, and a check that has not run for a while should not answer
/// with a quarter of a megabyte of build log. Past this it gets the newest and
/// is told how much it skipped — the same sentence a cursor that fell off the
/// buffer gets, because it is the same fact.
const MAX_CHECK_BYTES: usize = 64 * 1024;

/// The longest `check_command` will wait for a command to finish.
pub const MAX_WAIT_SECS: u64 = 120;

/// How long a finished command's output is waited for before it is called
/// finished anyway.
///
/// A child's own children inherit its pipes, so the write end stays open until
/// the last of them exits — a shell killed while its command runs on, or one
/// that started something and returned. Waiting for the read to end would mean
/// a command reported as running for as long as whatever it left behind, which
/// is not what a stop looks like. Late output is not lost: the drains go on
/// filling the buffer, and the next check reads it.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// One background command, as a surface that draws it needs it.
///
/// `status` is the sentence; the flags beside it are for a row that wants to
/// colour a failure differently from a stop. Nothing here is derived twice —
/// see [`say`].
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BackgroundJob {
    pub id: u32,
    /// The command line as it was given, which is the only name a job has.
    pub command: String,
    pub running: bool,
    /// Ended by a stop rather than by finishing. A stopped command has no
    /// meaningful code, and reading one as a failure is the mistake this
    /// prevents.
    pub stopped: bool,
    /// `None` while it runs, and after a signal killed it.
    #[ts(optional)]
    pub code: Option<i32>,
    /// Seconds so far, or seconds it took.
    ///
    /// `u32` rather than `u64` because it crosses to the window, where a
    /// 64-bit integer arrives as a `bigint` that no arithmetic beside it can
    /// use. A command running for longer than a hundred and thirty years is
    /// not the case this loses.
    pub ran_for: u32,
    pub status: String,
}

/// A reader's next helping of one command's output.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JobOutput {
    pub id: u32,
    pub text: String,
    /// Bytes between the cursor asked for and the oldest still here. Said
    /// rather than skipped over, so a pane that fell behind does not read a
    /// suffix as though it were the whole run.
    pub missed: usize,
    /// The cursor to ask with next time.
    pub cursor: usize,
}

/// Every background command of one workspace.
///
/// Held by the host rather than by a turn, because outliving the turn is the
/// whole point.
#[derive(Default)]
pub struct Jobs {
    /// Ordinary locks rather than the runtime's, and never held across an
    /// await: what is under them is a map lookup and a small enum, and a
    /// window closing has to be able to stop everything without an async
    /// context to do it in.
    jobs: Mutex<BTreeMap<u32, Arc<Job>>>,
    next: AtomicU32,
}

struct Job {
    id: u32,
    command: String,
    started: Instant,
    output: Mutex<Output>,
    /// `None` while it is running.
    outcome: Mutex<Option<Outcome>>,
    finished: Notify,
    stop: CancellationToken,
    /// The workspace as it stood when this started, spent when it exits.
    ///
    /// `None` when the caller keeps no checkpoints — a piped run, an example,
    /// a test — where there is no turn for a change to be recorded into.
    sweep: Mutex<Option<Sweep>>,
}

struct Outcome {
    code: Option<i32>,
    /// Ended by [`Jobs::stop`] rather than by finishing.
    stopped: bool,
    ran_for: Duration,
}

/// What a command has said, and where each reader has got to.
#[derive(Default)]
struct Output {
    tail: Tail,
    /// How far `check_command` has read.
    ///
    /// The window keeps its own, passed in and handed back, because neither
    /// reader may move the other's. See the module note.
    read: usize,
}

/// The end of a command's output, and a count of all of it.
///
/// A ring in effect rather than in structure: the last [`MAX_PENDING_BYTES`]
/// are kept, and `written` counts every byte that ever arrived — so `text`
/// holds the stream from `written - text.len()` onwards, and a reader is one
/// number in that space.
#[derive(Default)]
struct Tail {
    text: String,
    /// Every byte ever pushed, including the ones no longer here.
    written: usize,
}

impl Tail {
    fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        self.written += chunk.len();
        if self.text.len() > MAX_PENDING_BYTES {
            let over = self.text.len() - MAX_PENDING_BYTES;
            let cut = ceil_boundary(&self.text, over);
            self.text.drain(..cut);
        }
    }

    /// The oldest byte still here.
    fn held_from(&self) -> usize {
        self.written - self.text.len()
    }

    /// The bytes after `cursor` — at most `limit` of them — and how many
    /// between the two are gone.
    ///
    /// Output goes missing two ways and this counts both as one number,
    /// because they are the same fact to whoever asked: it fell off the front
    /// before this reader got to it, or there is more of it than this reader
    /// is willing to be handed at once.
    fn since(&self, cursor: usize, limit: usize) -> (&str, usize) {
        let held_from = self.held_from();
        let dropped = held_from.saturating_sub(cursor);
        // A cursor is handed out and handed back, so the clamp and the
        // rounding are for a caller that made one up — and an invented offset
        // slicing a string is the one way this answers with a panic.
        let from = ceil_boundary(
            &self.text,
            cursor.saturating_sub(held_from).min(self.text.len()),
        );
        let text = &self.text[from..];
        if text.len() <= limit {
            return (text, dropped);
        }
        let cut = ceil_boundary(text, text.len() - limit);
        (&text[cut..], dropped + cut)
    }
}

/// Rounds an index up to a character boundary, so a drained buffer stays valid
/// UTF-8 when the cut lands inside a multi-byte character.
fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

impl Jobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many are still running, for the caller that has to refuse a ninth.
    pub fn running(&self) -> usize {
        self.all()
            .iter()
            .filter(|job| job.outcome.lock().unwrap().is_none())
            .count()
    }

    /// Every job, out from under the lock.
    fn all(&self) -> Vec<Arc<Job>> {
        self.jobs.lock().unwrap().values().cloned().collect()
    }

    /// Takes over a started child, and returns the number the model will use.
    ///
    /// The child is moved rather than shared: one owner may wait on it, and
    /// stopping goes through [`Job::stop`] so that owner is the only one that
    /// ever reaps it.
    pub async fn adopt(&self, command: String, mut child: Child, sweep: Option<Sweep>) -> u32 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let job = Arc::new(Job {
            id,
            command,
            started: Instant::now(),
            output: Mutex::new(Output::default()),
            outcome: Mutex::new(None),
            finished: Notify::new(),
            stop: CancellationToken::new(),
            sweep: Mutex::new(sweep),
        });
        self.jobs.lock().unwrap().insert(id, job.clone());

        let drains = vec![
            drain(child.stdout.take(), job.clone()),
            drain(child.stderr.take(), job.clone()),
        ];
        let stop = job.stop.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                biased;
                _ = stop.cancelled() => {
                    // The group before the leader. A background command is a
                    // shell, and killing the shell alone leaves whatever it
                    // ran — the build, the watcher — still going, while this
                    // reports the command as stopped. See
                    // `crate::spawn::kill_group`.
                    crate::spawn::kill_group(child.id()).await;
                    let _ = child.start_kill();
                    child.wait().await
                }
                status = child.wait() => status,
            };
            // Before the outcome is published, so "it exited" also means "its
            // output is all here". A check that raced the exit would otherwise
            // report a finished command and lose its last lines — but bounded,
            // for the pipes nobody is going to close. See [`DRAIN_GRACE`].
            let _ = tokio::time::timeout(DRAIN_GRACE, async {
                for drain in drains {
                    let _ = drain.await;
                }
            })
            .await;
            *job.outcome.lock().unwrap() = Some(Outcome {
                code: status.ok().and_then(|s| s.code()),
                stopped: stop.is_cancelled(),
                ran_for: job.started.elapsed(),
            });
            job.finished.notify_waiters();
        });
        id
    }

    /// What one command has said since the last check, or a line about each of
    /// them when `id` is `None`.
    pub async fn check(&self, id: Option<u32>, wait: Duration) -> Result<String, String> {
        let Some(id) = id else {
            return Ok(self.roster());
        };
        let job = self.get(id)?;

        if !wait.is_zero() {
            // The future is created before the state is read: a command that
            // finishes in between has already notified, and waiting on a
            // notification that has been and gone is how a poll becomes a
            // two-minute stall.
            let finished = job.finished.notified();
            if job.outcome.lock().unwrap().is_none() {
                let _ = tokio::time::timeout(wait, finished).await;
            }
        }

        // The model's cursor moves; the window's is its own and untouched.
        let (text, missed) = {
            let mut output = job.output.lock().unwrap();
            let read = {
                let (text, missed) = output.tail.since(output.read, MAX_CHECK_BYTES);
                (text.to_string(), missed)
            };
            output.read = output.tail.written;
            read
        };
        let mut report = format!("#{} {} — {}", job.id, job.command, job.status());
        if missed > 0 {
            report.push_str(&format!(
                "\n[{missed} bytes of earlier output dropped; check more often to keep up]"
            ));
        }
        report.push('\n');
        if text.trim().is_empty() {
            report.push_str("(no new output)");
        } else {
            report.push_str(text.trim_end());
        }
        Ok(report)
    }

    /// Ends a command, and waits for it to actually be gone.
    pub async fn stop(&self, id: u32) -> Result<String, String> {
        let job = self.get(id)?;
        if job.outcome.lock().unwrap().is_some() {
            return Ok(format!(
                "#{} {} had already {}",
                job.id,
                job.command,
                job.status()
            ));
        }
        let finished = job.finished.notified();
        job.stop.cancel();
        // A kill the OS has not carried out yet is not a stopped command, and
        // the next call would sweep a workspace something is still writing to.
        let _ = tokio::time::timeout(Duration::from_secs(10), finished).await;
        Ok(format!("Stopped #{} {}", job.id, job.command))
    }

    /// Records what the commands that have finished since the last call
    /// changed, and returns anything the user needs told about it.
    ///
    /// Called after every tool call rather than only from `check_command`,
    /// because a model that never checks is exactly the case where the changes
    /// would otherwise go unrecorded.
    pub async fn reap(&self, workspace: &Path, recorder: &TurnRecorder) -> Vec<String> {
        let mut warnings = Vec::new();
        for job in self.all() {
            if job.outcome.lock().unwrap().is_none() {
                continue;
            }
            // Taken, so a job is swept once however often this runs.
            let Some(sweep) = job.sweep.lock().unwrap().take() else {
                continue;
            };
            if let Some(warning) = sweep.after(workspace, recorder).await.warning() {
                warnings.push(warning);
            }
        }
        warnings
    }

    /// Ends everything, for a window closing or a workspace being left.
    ///
    /// A background command belongs to the workspace it was started in, and
    /// nothing in the OS will tidy one up on the way out.
    pub fn stop_all(&self) {
        for job in self.all() {
            job.stop.cancel();
        }
    }

    /// Ends everything and forgets it, for a workspace being left.
    ///
    /// [`stop_all`](Self::stop_all) on its own leaves the roster standing,
    /// which is right for a window on its way out and wrong for a window that
    /// is staying and looking somewhere else. Two things go with the folder:
    /// the roster the model and the dock read, which would otherwise name
    /// commands that ran somewhere the window is no longer pointed, and the
    /// pre-image each job is holding — [`reap`](Self::reap) sweeps against
    /// *this* workspace, so a leftover job would compare a folder against a
    /// picture of a different one.
    ///
    /// The children still die: the task that reaps each one holds its own
    /// handle, so dropping the map here does not orphan a process.
    pub fn forget_all(&self) {
        self.stop_all();
        self.jobs.lock().unwrap().clear();
    }

    /// A line per command, for a model that has lost track of the numbers.
    pub fn roster(&self) -> String {
        let jobs = self.all();
        if jobs.is_empty() {
            return "No commands have been started in the background.".into();
        }
        jobs.iter()
            .map(|job| format!("#{} {} — {}", job.id, job.command, job.status()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every background command, for a surface that draws them.
    ///
    /// The window's half of [`roster`](Self::roster), which says the same
    /// things in a sentence for a model that has lost track of the numbers.
    pub fn list(&self) -> Vec<BackgroundJob> {
        self.all().iter().map(|job| job.view()).collect()
    }

    /// What one command has said after `cursor`, leaving the model's place in
    /// it where it was.
    ///
    /// `0` asks for everything still held, which is what a pane opened onto a
    /// command already running wants. The cursor to ask with next time comes
    /// back with the text.
    pub fn read(&self, id: u32, cursor: usize) -> Result<JobOutput, String> {
        let job = self.get(id)?;
        let output = job.output.lock().unwrap();
        let (text, missed) = output.tail.since(cursor, MAX_PENDING_BYTES);
        Ok(JobOutput {
            id,
            // The pane is a log, not a screen: a background command has no
            // pty, so almost nothing colours its output, and what does would
            // arrive as literal escape bytes in a block of text. Stripped for
            // this reader only — `check_command` gets what the command
            // actually wrote, which is what it has always got. A progress bar
            // that redrew its own line becomes one line per redraw, which is
            // the right shape for something scrolled rather than watched.
            text: crate::builtin::pty::strip_ansi(text),
            missed,
            cursor: output.tail.written,
        })
    }

    fn get(&self, id: u32) -> Result<Arc<Job>, String> {
        if let Some(job) = self.jobs.lock().unwrap().get(&id) {
            return Ok(job.clone());
        }
        Err(format!(
            "There is no background command #{id}.\n{}",
            self.roster()
        ))
    }
}

impl Job {
    fn status(&self) -> String {
        say(&self.outcome.lock().unwrap(), self.started)
    }

    fn view(&self) -> BackgroundJob {
        // One lock for all of it: a job read field by field could report
        // itself running with an exit code, which is a state it never was in.
        let outcome = self.outcome.lock().unwrap();
        BackgroundJob {
            id: self.id,
            command: self.command.clone(),
            running: outcome.is_none(),
            stopped: outcome.as_ref().is_some_and(|o| o.stopped),
            code: outcome.as_ref().and_then(|o| o.code),
            ran_for: outcome
                .as_ref()
                .map_or_else(|| self.started.elapsed(), |o| o.ran_for)
                .as_secs()
                .min(u32::MAX as u64) as u32,
            status: say(&outcome, self.started),
        }
    }
}

/// How a command is doing, in the one sentence both surfaces are given.
///
/// Said once so the window and `check_command` cannot describe the same
/// command differently — the flags beside it on [`BackgroundJob`] are for
/// styling a row, not for rewording this.
fn say(outcome: &Option<Outcome>, started: Instant) -> String {
    match outcome {
        None => format!("still running after {}", took(started.elapsed())),
        Some(outcome) if outcome.stopped => {
            format!("stopped after {}", took(outcome.ran_for))
        }
        Some(Outcome {
            code: Some(0),
            ran_for,
            ..
        }) => format!("finished after {}", took(*ran_for)),
        Some(Outcome {
            code: Some(code),
            ran_for,
            ..
        }) => format!("exited with code {code} after {}", took(*ran_for)),
        Some(Outcome { ran_for, .. }) => {
            format!("killed by a signal after {}", took(*ran_for))
        }
    }
}

/// Reads one of a child's pipes to end, into the job's buffer.
///
/// Bytes rather than lines of text, for the reason [`crate::builtin::shell`]
/// gives: a build log should not end at the first byte that is not UTF-8.
fn drain<R>(pipe: Option<R>, job: Arc<Job>) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let Some(pipe) = pipe else {
            return;
        };
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            job.output.lock().unwrap().tail.push(&text);
        }
    })
}

/// A duration as somebody would say it out loud.
fn took(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m{}s", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(command: &str) -> Child {
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/C".to_string(), command.to_string()])
        } else {
            ("sh", vec!["-c".to_string(), command.to_string()])
        };
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        // The same group the real background path gives a command, so what is
        // exercised here is what actually runs. See `shell::spawn_piped`.
        crate::spawn::own_group(&mut command);
        command.spawn().unwrap()
    }

    /// How many processes match `pattern` right now.
    #[cfg(unix)]
    fn matching(pattern: &str) -> usize {
        let out = std::process::Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .expect("pgrep");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_a_command_ends_what_it_started_and_not_only_the_shell() {
        /*
         * A background command is `sh -c "..."`, so the child is the shell and
         * the work is its child. `start_kill` reaches the shell alone — which
         * meant Stop ended the shell, left the build running, and reported the
         * command as stopped. This method's own doc says it waits for it to
         * actually be gone, and it did not.
         *
         * The marker is a sleep of an unusual length, so nothing else on the
         * machine can be mistaken for it.
         */
        const MARKER: &str = "31849";
        let jobs = Jobs::new();
        let id = jobs
            .adopt(
                format!("sleep {MARKER}"),
                sh(&format!("echo started; sleep {MARKER}")),
                None,
            )
            .await;

        // Started, before there is anything to assert about stopping it.
        let mut running = 0;
        for _ in 0..100 {
            running = matching(&format!("sleep {MARKER}"));
            if running > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(running > 0, "the command never started");

        jobs.stop(id).await.unwrap();

        let mut left = usize::MAX;
        for _ in 0..100 {
            left = matching(&format!("sleep {MARKER}"));
            if left == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Tidied either way, so a failure does not strand a process for the
        // next run to trip over.
        let _ = std::process::Command::new("pkill")
            .args(["-f", &format!("sleep {MARKER}")])
            .output();
        assert_eq!(left, 0, "the command's own child outlived Stop");
    }

    #[tokio::test]
    async fn a_finished_command_reports_its_output_and_its_code() {
        let jobs = Jobs::new();
        let id = jobs.adopt("echo hi".into(), sh("echo hi"), None).await;
        let report = jobs.check(Some(id), Duration::from_secs(10)).await.unwrap();
        assert!(report.contains("finished after"), "{report}");
        assert!(report.contains("hi"), "{report}");
    }

    #[tokio::test]
    async fn output_is_read_once() {
        let jobs = Jobs::new();
        let id = jobs.adopt("echo hi".into(), sh("echo hi"), None).await;
        let _ = jobs.check(Some(id), Duration::from_secs(10)).await.unwrap();
        let second = jobs.check(Some(id), Duration::ZERO).await.unwrap();
        assert!(second.contains("(no new output)"), "{second}");
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_code() {
        let jobs = Jobs::new();
        let id = jobs.adopt("exit 3".into(), sh("exit 3"), None).await;
        let report = jobs.check(Some(id), Duration::from_secs(10)).await.unwrap();
        assert!(report.contains("exited with code 3"), "{report}");
    }

    /// Long enough that nothing here can outrun it, and bounded so a stop that
    /// leaves the shell's own child behind on Windows does not leave it for
    /// long. `cmd` has no `sleep`, which is why this is not one string.
    #[cfg(windows)]
    const KEEPS_RUNNING: &str = "ping -n 31 127.0.0.1 > nul";
    #[cfg(not(windows))]
    const KEEPS_RUNNING: &str = "sleep 30";

    #[tokio::test]
    async fn a_command_still_running_says_so_and_stops_on_request() {
        let jobs = Jobs::new();
        let id = jobs
            .adopt(KEEPS_RUNNING.into(), sh(KEEPS_RUNNING), None)
            .await;
        let report = jobs.check(Some(id), Duration::ZERO).await.unwrap();
        assert!(report.contains("still running"), "{report}");
        assert_eq!(jobs.running(), 1);

        let stopped = jobs.stop(id).await.unwrap();
        assert!(stopped.starts_with("Stopped #"), "{stopped}");
        assert_eq!(jobs.running(), 0);
        let after = jobs.check(Some(id), Duration::ZERO).await.unwrap();
        assert!(after.contains("stopped after"), "{after}");
    }

    #[tokio::test]
    async fn waiting_returns_as_soon_as_it_finishes() {
        let jobs = Jobs::new();
        let id = jobs.adopt("echo done".into(), sh("echo done"), None).await;
        let started = Instant::now();
        let report = jobs.check(Some(id), Duration::from_secs(30)).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "waited too long"
        );
        assert!(report.contains("done"), "{report}");
    }

    /// Unix only for the shape of the command: a shell that starts something
    /// in the background and returns leaves that child holding the pipes, and
    /// `cmd` has no equivalent one-liner. What it guards is not
    /// platform-specific at all — killing a shell on Windows leaves its own
    /// command holding them the same way.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_pipe_nobody_will_close_does_not_hold_the_command_open() {
        let jobs = Jobs::new();
        let id = jobs
            .adopt("leaves one behind".into(), sh("sleep 30 &"), None)
            .await;

        // The shell is gone in an instant; what it started holds the write end
        // of both pipes for half a minute.
        let waited = Instant::now();
        let report = jobs.check(Some(id), Duration::from_secs(20)).await.unwrap();
        assert!(
            waited.elapsed() < Duration::from_secs(10),
            "waited {:?} for a command that had already exited",
            waited.elapsed()
        );
        assert!(report.contains("finished after"), "{report}");
        assert_eq!(jobs.running(), 0);
    }

    #[tokio::test]
    async fn an_unknown_number_lists_the_ones_that_exist() {
        let jobs = Jobs::new();
        let id = jobs.adopt("echo hi".into(), sh("echo hi"), None).await;
        let err = jobs.check(Some(id + 7), Duration::ZERO).await.unwrap_err();
        assert!(err.contains("no background command"), "{err}");
        assert!(err.contains("echo hi"), "{err}");
    }

    #[tokio::test]
    async fn the_roster_is_empty_before_anything_starts() {
        let jobs = Jobs::new();
        assert!(jobs
            .check(None, Duration::ZERO)
            .await
            .unwrap()
            .contains("No commands"));
    }

    #[tokio::test]
    async fn the_window_reads_a_command_without_taking_it_from_the_model() {
        // The whole reason this file has cursors. A pane drawing a build used
        // to be a pane emptying the buffer `check_command` was going to read.
        let jobs = Jobs::new();
        let id = jobs.adopt("echo hi".into(), sh("echo hi"), None).await;
        let _ = jobs.check(Some(id), Duration::from_secs(10)).await.unwrap();

        let seen = jobs.read(id, 0).unwrap();
        assert!(seen.text.contains("hi"), "{seen:?}");
        assert_eq!(seen.missed, 0);

        // And the reverse: the window having read it does not make the next
        // check say there was nothing.
        let again = jobs.read(id, 0).unwrap();
        assert_eq!(again.text, seen.text);
    }

    #[tokio::test]
    async fn a_window_cursor_only_asks_for_what_it_has_not_seen() {
        let jobs = Jobs::new();
        let id = jobs
            .adopt("echo one; echo two".into(), sh("echo one; echo two"), None)
            .await;
        jobs.check(Some(id), Duration::from_secs(10)).await.unwrap();

        let first = jobs.read(id, 0).unwrap();
        assert!(first.text.contains("one"), "{first:?}");
        let next = jobs.read(id, first.cursor).unwrap();
        assert_eq!(next.text, "");
        assert_eq!(next.cursor, first.cursor);
    }

    #[tokio::test]
    async fn a_job_lists_itself_as_running_and_then_as_finished() {
        let jobs = Jobs::new();
        let id = jobs.adopt("exit 3".into(), sh("exit 3"), None).await;
        jobs.check(Some(id), Duration::from_secs(10)).await.unwrap();

        let listed = jobs.list();
        assert_eq!(listed.len(), 1);
        let job = &listed[0];
        assert_eq!(job.id, id);
        assert_eq!(job.command, "exit 3");
        assert!(!job.running);
        assert!(!job.stopped);
        assert_eq!(job.code, Some(3));
        // The sentence is the one `check_command` was given, not a second
        // wording of it.
        assert!(job.status.contains("exited with code 3"), "{job:?}");
    }

    #[tokio::test]
    async fn reading_a_number_that_is_not_there_says_which_ones_are() {
        let jobs = Jobs::new();
        jobs.adopt("echo hi".into(), sh("echo hi"), None).await;
        let err = jobs.read(99, 0).unwrap_err();
        assert!(err.contains("no background command"), "{err}");
    }

    #[tokio::test]
    async fn the_window_is_given_text_rather_than_escape_bytes() {
        // No pty means most commands do not colour at all, but the ones told
        // to anyway would otherwise draw their escapes into the pane. The
        // model's copy is untouched.
        let jobs = Jobs::new();
        let coloured = "printf '\\033[32mgreen\\033[0m\\n'";
        let id = jobs.adopt(coloured.into(), sh(coloured), None).await;
        jobs.check(Some(id), Duration::from_secs(10)).await.unwrap();
        let seen = jobs.read(id, 0).unwrap();
        assert_eq!(seen.text.trim(), "green", "{seen:?}");
    }

    #[test]
    fn output_past_the_buffer_drops_the_oldest() {
        let mut tail = Tail::default();
        tail.push(&"a".repeat(MAX_PENDING_BYTES));
        tail.push("bbbb");
        let (text, missed) = tail.since(0, MAX_PENDING_BYTES);
        assert_eq!(missed, 4);
        assert_eq!(text.len(), MAX_PENDING_BYTES);
        assert!(text.ends_with("bbbb"));
    }

    #[test]
    fn dropping_output_does_not_split_a_character() {
        let mut tail = Tail::default();
        tail.push(&"é".repeat(MAX_PENDING_BYTES));
        tail.push("x");
        let (text, _) = tail.since(0, MAX_PENDING_BYTES);
        assert!(text.ends_with('x'));
    }

    #[test]
    fn a_reader_that_asks_for_less_is_given_the_newest_of_it() {
        // What keeps a check from answering with a quarter megabyte of build
        // log: the model gets the end, and is told what it skipped.
        let mut tail = Tail::default();
        tail.push(&"a".repeat(100));
        tail.push("end");
        let (text, missed) = tail.since(0, 3);
        assert_eq!(text, "end");
        assert_eq!(missed, 100);
    }

    #[test]
    fn a_cursor_from_nowhere_is_answered_rather_than_panicked_on() {
        // Nothing hands one out, but it crosses a process boundary to get
        // here, and slicing a string at an invented offset is the one way this
        // could answer with a crash.
        let mut tail = Tail::default();
        tail.push("héllo");
        assert_eq!(tail.since(9_000, MAX_PENDING_BYTES).0, "");
        // Inside a multi-byte character: rounded up rather than split.
        assert_eq!(tail.since(2, MAX_PENDING_BYTES).0, "llo");
    }
}
