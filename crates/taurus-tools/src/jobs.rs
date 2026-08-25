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
//! drained into a buffer as it arrives and handed over whole at the next
//! check. The buffer is [`MAX_PENDING_BYTES`], and past that the oldest is
//! dropped and counted — a check that arrives late learns that it did, rather
//! than reading a prefix as though it were everything. Both streams go into
//! the one buffer in the order they arrived: a background command is watched
//! rather than parsed, and that is the order a terminal would have shown.
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

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::checkpoint::TurnRecorder;
use crate::sweep::Sweep;

/// How many commands may be running in the background at once.
///
/// Not a resource limit — eight `cargo build`s would be one machine's problem
/// either way. It is a limit on how much a model can lose track of: every one
/// of these is a process the user did not start and cannot see, and a roster
/// that needs scrolling is one nobody stops.
pub const MAX_JOBS: usize = 8;

/// Output held for one job between checks.
const MAX_PENDING_BYTES: usize = 64 * 1024;

/// The longest `check_command` will wait for a command to finish.
pub const MAX_WAIT_SECS: u64 = 120;

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
    pending: Mutex<Pending>,
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

/// Output that has arrived and not yet been read.
#[derive(Default)]
struct Pending {
    text: String,
    /// Bytes dropped off the front to stay under the cap.
    dropped: usize,
}

impl Pending {
    fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        if self.text.len() > MAX_PENDING_BYTES {
            let over = self.text.len() - MAX_PENDING_BYTES;
            let cut = ceil_boundary(&self.text, over);
            self.dropped += cut;
            self.text.drain(..cut);
        }
    }

    fn take(&mut self) -> (String, usize) {
        (
            std::mem::take(&mut self.text),
            std::mem::replace(&mut self.dropped, 0),
        )
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
            pending: Mutex::new(Pending::default()),
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
                    let _ = child.start_kill();
                    child.wait().await
                }
                status = child.wait() => status,
            };
            // Before the outcome is published, so "it exited" also means "its
            // output is all here". A check that raced the exit would otherwise
            // report a finished command and lose its last lines.
            for drain in drains {
                let _ = drain.await;
            }
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

        let (text, dropped) = job.pending.lock().unwrap().take();
        let mut report = format!("#{} {} — {}", job.id, job.command, job.status());
        if dropped > 0 {
            report.push_str(&format!(
                "\n[{dropped} bytes of earlier output dropped; check more often to keep up]"
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
        match &*self.outcome.lock().unwrap() {
            None => format!("still running after {}", took(self.started.elapsed())),
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
            job.pending.lock().unwrap().push(&text);
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
        tokio::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
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

    #[tokio::test]
    async fn a_command_still_running_says_so_and_stops_on_request() {
        let jobs = Jobs::new();
        let id = jobs.adopt("sleep 60".into(), sh("sleep 60"), None).await;
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

    #[test]
    fn pending_output_drops_the_oldest_and_counts_it() {
        let mut pending = Pending::default();
        pending.push(&"a".repeat(MAX_PENDING_BYTES));
        pending.push("bbbb");
        let (text, dropped) = pending.take();
        assert_eq!(dropped, 4);
        assert_eq!(text.len(), MAX_PENDING_BYTES);
        assert!(text.ends_with("bbbb"));
    }

    #[test]
    fn dropping_output_does_not_split_a_character() {
        let mut pending = Pending::default();
        pending.push(&"é".repeat(MAX_PENDING_BYTES / 2));
        pending.push("x");
        let (text, _) = pending.take();
        assert!(text.ends_with('x'));
    }
}
