//! Shell execution.
//!
//! Two paths. By default a command gets three pipes and no stdin, which is right
//! for almost everything an agent runs: a model cannot answer a `[y/N]` prompt,
//! so a command that waits for one must fail on the timeout rather than hang the
//! session forever.
//!
//! The exception is the program that asks whether it is talking to a terminal.
//! Told no, `git` pages and colors nothing, `npm create` declines to scaffold,
//! and a full-screen prompt library fails at startup — behavior a person would
//! never see and cannot easily explain. Passing `pty` runs the command under a
//! real pseudo-terminal instead, and `stdin` hands it the answers up front,
//! which is what turns "behaves correctly" into "completes". See [`super::pty`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::warn;

use crate::budget::OutputBudget;
use crate::tool::{
    parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolProgress, ToolResult,
};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
/// What one command's output may take of the model's context window.
///
/// A share rather than a size: 64 KB is about sixteen thousand tokens, which
/// is twice what an 8k local model can hold and under two percent of a
/// million-token one. At [`OutputBudget::ANCHOR_WINDOW`] the share below comes
/// to the 64 KB this was before, so the model these numbers were chosen
/// against sees no change.
const OUTPUT_SHARE: f32 = 0.08;
/// Below this a cut stops bounding output and starts destroying it: a model
/// shown three lines of a build log has learned nothing and runs it again.
const MIN_OUTPUT_BYTES: usize = 4 * 1024;
/// Past this a single command's output is a different problem than a budget,
/// however much room the window has.
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

/// How many cut streams one workspace keeps on disk.
///
/// Enough that a model can still reach back several turns for the middle of a
/// build it was shown the ends of; few enough that a directory of logs never
/// grows into something somebody has to go and notice. Trimmed on the way to
/// writing one, which is the only moment this code runs at all.
const KEPT_SPILLS: usize = 20;

/// How long output may pool before it reaches the screen.
///
/// The point of streaming is that a slow command looks alive, and a tenth of a
/// second is under what reads as a delay. Sending every line the instant it
/// arrives would instead put a `cargo build` through the IPC channel one line
/// at a time, which is thousands of messages to draw the same text.
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// How much may pool within one interval before it is sent early, so a burst
/// of output does not sit waiting on a timer.
const FLUSH_BYTES: usize = 8 * 1024;

/// Lines held for the UI while a batch is in flight. Beyond this the display
/// drops output rather than making the child wait — see [`spawn_stream`].
pub(super) const STREAM_BACKLOG: usize = 512;

#[derive(Deserialize, JsonSchema)]
pub struct RunCommandInput {
    /// The command line to run, as you would type it in a terminal.
    pub command: String,
    /// Working directory, relative to the workspace root. Defaults to the root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Seconds before the command is killed. Defaults to 120, maximum 600.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Run under a pseudo-terminal, so the command believes it is talking to a
    /// terminal. Combines stdout and stderr into one stream.
    #[serde(default)]
    pub pty: bool,
    /// Text to feed the command's input, followed by end-of-file.
    #[serde(default)]
    pub stdin: Option<String>,
    /// Start the command and return without waiting for it, so a long build,
    /// a full test run, or a server can carry on while you work. Read what it
    /// has said with `check_command`, and end it with `stop_command`.
    #[serde(default)]
    pub background: bool,
}

pub struct RunCommand;

#[async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Run a shell command and return its output. It starts in the workspace root, so write \
         paths relative to it and do not cd first — use the cwd argument for a subdirectory. By \
         default it runs with no stdin, so prefer flags like -y over expecting a prompt. Set pty \
         to true for a command that behaves differently outside a terminal — git without a pager, \
         npm create, anything that draws a full-screen prompt — and pass stdin to answer prompts \
         it still asks. Set background to true for something that takes longer than the timeout \
         allows or is meant to keep running — a build from cold, a whole test suite, a dev server \
         — and read it later with check_command. Prefer read_file, glob, and grep over cat, find, \
         and grep -r."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<RunCommandInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn touches_unpredictably(&self) -> bool {
        true
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let command = input
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("?")
            .trim();
        // The user is approving this exact command line, so show it in full up
        // to a sane width rather than an elided summary.
        let shown: String = command.chars().take(300).collect();
        if input.get("background").and_then(|b| b.as_bool()) == Some(true) {
            return format!("Run in the background: {shown}");
        }
        format!("Run: {shown}")
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: RunCommandInput = parse_input(input)?;
        if input.command.trim().is_empty() {
            return Err(ToolError::InvalidInput("command must not be empty".into()));
        }

        let cwd = ctx.resolve(input.cwd.as_deref().unwrap_or("."))?;
        let timeout = Duration::from_secs(
            input
                .timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        );

        let (program, args) = shell_invocation(&input.command);

        if input.background {
            return start_in_background(&input, program, args, cwd, ctx).await;
        }

        // Set when a terminal was asked for and could not be had, so the result
        // can say so. Running the command anyway is right — the caller wanted
        // the command, and the terminal was how they hoped to get it — but
        // doing that silently would be worse than failing: the model would read
        // `git`'s piped output as what `git` does under a terminal and conclude
        // the wrong thing about the machine it is on.
        let mut no_terminal: Option<String> = None;

        if input.pty {
            match crate::builtin::pty::run(
                program.clone(),
                &args,
                &cwd,
                input.stdin.clone(),
                timeout,
                ctx.cancel.clone(),
                ctx.progress.clone(),
            )
            .await
            {
                Ok(output) => {
                    return Ok(report_for(
                        output.exit_code,
                        // One stream, so it is named for what it is rather
                        // than for a half of the pair it does not have.
                        &for_the_model(&output.text, "output", ctx),
                        // A terminal has one stream, so there is no stderr to
                        // label. Saying so keeps the model from reading its
                        // absence as the command having written nothing to it.
                        None,
                    )
                    .into());
                }
                // The command itself went wrong, or was canceled, or timed out.
                // Re-running it with pipes would run it twice.
                Err(crate::builtin::pty::PtyError::Failed(error)) => return Err(error),
                // No terminal to be had on this machine. Fall through and run
                // the command the ordinary way rather than failing a call that
                // has nothing wrong with it.
                Err(crate::builtin::pty::PtyError::Unavailable(reason)) => {
                    warn!(%reason, "no pseudo-terminal available; running with pipes");
                    no_terminal = Some(reason);
                }
            }
        }

        let mut child = spawn_piped(program, args, &cwd, input.stdin.is_some())?;
        feed_stdin(&mut child, input.stdin.as_deref()).await;

        // Drain the pipes concurrently with the wait. A child that fills its
        // stdout buffer blocks forever if nobody is reading, which would turn
        // every chatty command into a timeout.
        //
        // Both streams report to the same place and interleave there, which is
        // what a terminal shows and what the user is picturing. The split back
        // into stdout and stderr is kept for the model, which is reading the
        // result rather than watching it.
        let drain_stdout = spawn_stream(child.stdout.take(), ctx.progress.clone());
        let drain_stderr = spawn_stream(child.stderr.take(), ctx.progress.clone());

        let status = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                let _ = child.start_kill();
                return Err(ToolError::Canceled);
            }
            result = tokio::time::timeout(timeout, child.wait()) => result,
        };

        let status = match status {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(ToolError::Failed(e.to_string())),
            Err(_) => {
                let _ = child.start_kill();
                return Err(ToolError::Failed(format!(
                    "Command timed out after {}s and was killed. If it needs longer, raise \
                     timeout_secs; if it was waiting for input, rerun it non-interactively.",
                    timeout.as_secs()
                )));
            }
        };

        let code = status.code();
        let stdout = for_the_model(&drain_stdout.await, "stdout", ctx);
        let stderr = for_the_model(&drain_stderr.await, "stderr", ctx);
        let mut report = report_for(code, &stdout, Some(&stderr));

        // Said in the result rather than only in a log, because the model is
        // the one that has to account for it.
        if let Some(reason) = no_terminal {
            report = with_no_terminal_note(&reason, &report);
        }
        Ok(report.into())
    }
}

/// Prefixes a command's output with the fact that it did not get the terminal
/// it asked for.
///
/// Above the output rather than below it. A program told it is not on a
/// terminal pages nothing, colours nothing, and declines some prompts outright
/// — all of which read as facts about the project unless the reader already
/// knows the terminal never arrived, and a caveat underneath is read after the
/// conclusion has been drawn.
fn with_no_terminal_note(reason: &str, report: &str) -> String {
    format!(
        "Note: a terminal was requested but none could be opened here ({reason}), so this ran \
         with ordinary pipes. The command may behave as it does when piped rather than when run \
         in a terminal.\n\n{report}"
    )
}

/// Assembles what the model reads from a finished command.
///
/// Shared by both paths so an exit code means the same thing however the
/// command was run. `stderr` is `None` under a pseudo-terminal, where there is
/// only one stream to have.
///
/// A non-zero exit is information for the model, not a harness failure:
/// returning it as `Ok` lets the model read the compiler errors it just asked
/// for instead of a bare error string.
fn report_for(code: Option<i32>, stdout: &str, stderr: Option<&str>) -> String {
    let mut report = String::new();
    if !stdout.trim().is_empty() {
        report.push_str(stdout);
    }
    if let Some(stderr) = stderr.filter(|s| !s.trim().is_empty()) {
        if !report.is_empty() {
            report.push('\n');
        }
        report.push_str("[stderr]\n");
        report.push_str(stderr);
    }
    if report.trim().is_empty() {
        report.push_str("(no output)");
    }

    match code {
        Some(0) => report,
        Some(code) => format!("Exit code {code}\n{report}"),
        None => format!("Killed by signal\n{report}"),
    }
}

/// Starts a child with three pipes, the way both paths want it.
fn spawn_piped(
    program: String,
    args: Vec<String>,
    cwd: &std::path::Path,
    stdin: bool,
) -> Result<tokio::process::Child, ToolError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Without this the child outlives a canceled turn and keeps writing. A
    // background command is held by its own task rather than dropped at the
    // end of the call, so this does not end one early — it is what ends them
    // all when the runtime goes away.
    command.kill_on_drop(true);
    // Taurus has no console of its own, so on Windows starting `cmd` here
    // would open one — a black window flashing up on every command.
    crate::spawn::no_console(&mut command);
    command
        .spawn()
        .map_err(|e| ToolError::Failed(format!("cannot start shell: {e}")))
}

/// Written and closed before the output is drained. A program waiting on input
/// needs the end-of-file as much as the bytes.
async fn feed_stdin(child: &mut tokio::process::Child, text: Option<&str>) {
    let Some(text) = text else {
        return;
    };
    if let Some(mut pipe) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = pipe.write_all(text.as_bytes()).await;
        let _ = pipe.shutdown().await;
    }
}

/// Starts a command that will outlive this call.
///
/// The two arguments that only mean something to a command being waited for
/// are refused rather than ignored: a timeout nothing enforces and a terminal
/// nothing is watching are both answers to a question the caller asked, and
/// silently not honoring one is how a model concludes the wrong thing about
/// what it just started.
async fn start_in_background(
    input: &RunCommandInput,
    program: String,
    args: Vec<String>,
    cwd: std::path::PathBuf,
    ctx: &ToolContext,
) -> ToolResult {
    let Some(jobs) = &ctx.jobs else {
        return Err(ToolError::Rejected(
            "background commands are not available in this run; run it in the foreground, or \
             split it into steps that finish inside the timeout"
                .into(),
        ));
    };
    if input.pty {
        return Err(ToolError::InvalidInput(
            "a background command cannot have a pseudo-terminal. Run it in the foreground with \
             pty, or in the background without it."
                .into(),
        ));
    }
    if input.timeout_secs.is_some() {
        return Err(ToolError::InvalidInput(
            "timeout_secs does not apply to a background command — nothing is waiting for it. \
             End it with stop_command when you are done with it."
                .into(),
        ));
    }
    let running = jobs.running();
    if running >= crate::jobs::MAX_JOBS {
        return Err(ToolError::Rejected(format!(
            "{running} commands are already running in the background, which is the limit. Stop \
             one with stop_command first."
        )));
    }

    // Before the command starts, and held by the job until it exits: what it
    // changes is minutes away and in some later turn, and a pre-image read
    // then would be of a file the command had already written. See
    // [`crate::jobs`].
    let sweep = match &ctx.checkpoints {
        Some(_) => Some(crate::sweep::Sweep::before(&ctx.workspace, ctx.sweeps.clone()).await),
        None => None,
    };

    let mut child = spawn_piped(program, args, &cwd, input.stdin.is_some())?;
    feed_stdin(&mut child, input.stdin.as_deref()).await;
    let id = jobs.adopt(input.command.clone(), child, sweep).await;

    Ok(format!(
        "Started #{id} in the background: {}\nRead what it says with check_command (id {id}), \
         and end it with stop_command. It keeps running between turns.",
        input.command.trim()
    )
    .into())
}

#[derive(Deserialize, JsonSchema)]
pub struct CheckCommandInput {
    /// The number `run_command` gave you when it started. Omit to list every
    /// background command and how each is doing.
    #[serde(default)]
    pub id: Option<u32>,
    /// Wait up to this many seconds for it to finish, returning the moment it
    /// does. Defaults to not waiting at all. Maximum 120.
    #[serde(default)]
    pub wait_secs: Option<u64>,
}

pub struct CheckCommand;

#[async_trait]
impl Tool for CheckCommand {
    fn name(&self) -> &str {
        "check_command"
    }

    fn description(&self) -> &str {
        "Read what a background command has said since you last checked, and whether it is still \
         running. Output arrives once — what you read here you will not be shown again — and both \
         of its streams are merged in the order they arrived. Pass wait_secs to wait for it to \
         finish instead of asking again in a moment. Omit id to see every background command at \
         once."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<CheckCommandInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    /// The test run this reads is the check on work already written — see
    /// [`Tool::checks_work`].
    fn checks_work(&self) -> bool {
        true
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        match input.get("id").and_then(|i| i.as_u64()) {
            Some(id) => format!("Check background command #{id}"),
            None => "List background commands".into(),
        }
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: CheckCommandInput = parse_input(input)?;
        let jobs = jobs_of(ctx)?;
        let wait =
            Duration::from_secs(input.wait_secs.unwrap_or(0).min(crate::jobs::MAX_WAIT_SECS));
        // Cancellation reaches the wait rather than the command: the user
        // stopping a turn wants the turn back, not the build killed.
        let report = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(ToolError::Canceled),
            report = jobs.check(input.id, wait) => report,
        };
        report.map(Into::into).map_err(ToolError::InvalidInput)
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct StopCommandInput {
    /// The number `run_command` gave you when it started.
    pub id: u32,
}

pub struct StopCommand;

#[async_trait]
impl Tool for StopCommand {
    fn name(&self) -> &str {
        "stop_command"
    }

    fn description(&self) -> &str {
        "End a background command. Anything it has written that you have not read is lost with \
         it, so check_command first if the output mattered."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<StopCommandInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        match input.get("id").and_then(|i| i.as_u64()) {
            Some(id) => format!("Stop background command #{id}"),
            None => "Stop a background command".into(),
        }
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: StopCommandInput = parse_input(input)?;
        jobs_of(ctx)?
            .stop(input.id)
            .await
            .map(Into::into)
            .map_err(ToolError::InvalidInput)
    }
}

/// The background commands, or the refusal to pretend there are any.
fn jobs_of(ctx: &ToolContext) -> Result<&Arc<crate::jobs::Jobs>, ToolError> {
    ctx.jobs.as_ref().ok_or_else(|| {
        ToolError::Rejected("background commands are not available in this run".into())
    })
}

/// Reads a child pipe to end in the background, reporting it as it arrives.
///
/// Two jobs at once, and they have different standards. The returned text is
/// what the model gets, so it must be complete. What goes to `progress` is what
/// the user watches scroll past, so it must be prompt — and may be incomplete,
/// because the alternative is worse: a display that cannot keep up would
/// otherwise stall the reader, fill the child's pipe buffer, and hang the
/// command. Lines are dropped from the view before that is allowed to happen.
///
/// Read as bytes rather than as lines of text, because a command is free to
/// emit something that is not UTF-8 and a build log should not end at the first
/// byte that isn't.
fn spawn_stream<R>(
    pipe: Option<R>,
    progress: Option<Arc<dyn ToolProgress>>,
) -> impl std::future::Future<Output = String>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<String>(STREAM_BACKLOG);
    if let Some(progress) = progress {
        tokio::spawn(batch_to_progress(rx, progress));
    }

    let handle = tokio::spawn(async move {
        let mut full = String::new();
        let Some(pipe) = pipe else {
            return full;
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
            full.push_str(&text);
            // Never `send`, which would wait. A full channel means the UI is
            // behind; the model's copy is already safe in `full`.
            let _ = tx.try_send(text);
        }
        full
    });

    async move { handle.await.unwrap_or_default() }
}

/// Collects streamed lines into batches and reports each one.
pub(super) async fn batch_to_progress(
    mut rx: mpsc::Receiver<String>,
    progress: Arc<dyn ToolProgress>,
) {
    let mut pending = String::new();
    loop {
        match tokio::time::timeout(FLUSH_INTERVAL, rx.recv()).await {
            Ok(Some(line)) => {
                pending.push_str(&line);
                if pending.len() >= FLUSH_BYTES {
                    progress.step(std::mem::take(&mut pending)).await;
                }
            }
            // The pipe closed. Whatever is left is the tail of the output, and
            // is the part most worth seeing.
            Ok(None) => {
                if !pending.is_empty() {
                    progress.step(pending).await;
                }
                return;
            }
            // The interval elapsed. A command that prints one line and then
            // thinks for a minute must show that line now, not on its next.
            Err(_) => {
                if !pending.is_empty() {
                    progress.step(std::mem::take(&mut pending)).await;
                }
            }
        }
    }
}

/// The shell to run a command line through on this platform.
#[cfg(windows)]
fn shell_invocation(command: &str) -> (String, Vec<String>) {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    (shell, vec!["/C".into(), command.to_string()])
}

#[cfg(not(windows))]
fn shell_invocation(command: &str) -> (String, Vec<String>) {
    // Deliberately /bin/sh rather than $SHELL: the model writes POSIX command
    // lines, and a user's fish or nushell login shell would not parse them.
    ("/bin/sh".into(), vec!["-c".into(), command.to_string()])
}

/// What the model reads of one stream, and where the rest of it went.
///
/// Three things in order. Repetition is collapsed first, because a stream
/// that says the same thing forty times is long for a reason a byte count
/// cannot see — and doing it here often means the cut below never fires.
/// What is left is cut if it is still too long, keeping the tail as well as
/// the head: errors and summaries live at the end. And the stream is written
/// out **as it arrived**, repeats and all, so that the gap is something the
/// model can go and read rather than something it has to run the command
/// again to see.
///
/// The file holds the original rather than the shortened version on purpose.
/// A collapsed run says how many lines stood there and what they said, so
/// nothing is lost by reading it — but the file is the record of what the
/// command actually printed, and a record that had been edited first would be
/// a worse thing to keep.
fn for_the_model(raw: &str, stream: &str, ctx: &ToolContext) -> String {
    let condensed = super::condense::condense(raw, ctx.budget);
    let shown = condensed.as_deref().unwrap_or(raw);
    let cap = output_cap(ctx.budget);
    if shown.len() <= cap {
        return shown.to_string();
    }
    let head_len = cap * 2 / 3;
    let head = floor_boundary(shown, head_len);
    let tail_start = shown.len() - (cap - head_len);
    let tail = ceil_boundary(shown, tail_start);
    format!(
        "{}\n\n[… {} …]\n\n{}",
        &shown[..head],
        elision(shown.len() - cap, raw, stream, ctx),
        &shown[tail..]
    )
}

/// What one command's output may take, for the model this turn is on.
fn output_cap(budget: OutputBudget) -> usize {
    budget.bytes(OUTPUT_SHARE, MIN_OUTPUT_BYTES, MAX_OUTPUT_BYTES)
}

/// The sentence in the gap: how much went, and where it still is.
///
/// The path is the whole point of it. Without one the only route back to the
/// middle of a long build is to run the build again — minutes, and a second
/// set of side effects, to look at something that already happened.
fn elision(omitted: usize, raw: &str, stream: &str, ctx: &ToolContext) -> String {
    let Some(path) = spill(raw, stream, ctx) else {
        return format!("{omitted} bytes omitted");
    };
    // `read_file` at any size: it windows around the offset it is given
    // rather than reading a prefix, so the middle of a large log is a call
    // away and not a search away.
    format!(
        "{omitted} bytes omitted; this stream was written out whole to {} — read_file it \
         to see them",
        path.display()
    )
}

/// Writes a stream out whole and says where it went.
///
/// `None` when there is nowhere to put it, or the write failed, and both are
/// silent on purpose. The command ran. Losing the copy costs the model a
/// second look at the middle, and turning that into a failed tool call would
/// throw away the result along with it.
fn spill(text: &str, stream: &str, ctx: &ToolContext) -> Option<PathBuf> {
    let dir = ctx.command_output.as_ref()?;
    std::fs::create_dir_all(dir).ok()?;
    // Before the write rather than after, so the directory is at its bound
    // once this one lands rather than one over it until the next command runs.
    prune(dir, KEPT_SPILLS.saturating_sub(1));
    let path = dir.join(format!(
        "{}-{}-{}.txt",
        slug(ctx.session_id.as_deref().unwrap_or("session")),
        slug(ctx.call_id.as_deref().unwrap_or("command")),
        stream
    ));
    std::fs::write(&path, text).ok()?;
    // Canonicalized because this is about to be handed back as a path to read,
    // and the guard that decides whether it may be read canonicalizes both
    // sides before comparing them.
    path.canonicalize().ok()
}

/// Keeps the newest `keep` files in a directory and deletes the rest.
///
/// Every failure here is ignored. This is tidying, and a directory that
/// cannot be tidied is not a reason to fail the command whose output was
/// about to go into it.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, entry.path()))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    // Newest first, so what survives is the head of the list.
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

/// A session or call id as a filename component.
///
/// Both are ids this process was handed rather than ids it chose — a
/// provider names the call — so nothing guarantees they are made of
/// characters a path may contain.
fn slug(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    cleaned.trim_matches('-').chars().take(64).collect()
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;
    use tempfile::TempDir;

    /// Unix-only: these assert on shell behavior that `cmd.exe` does not share,
    /// and the pty path itself is covered per-platform in [`super::pty`].
    #[tokio::test]
    async fn a_pty_command_is_told_it_has_a_terminal() {
        if cfg!(windows) {
            return;
        }
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({
                    "command": "test -t 1 && echo tty || echo pipe",
                    "pty": true,
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("tty"), "{out}");
    }

    #[tokio::test]
    async fn without_a_pty_the_same_command_is_told_it_is_a_pipe() {
        // The default has to stay exactly what it was: almost everything an
        // agent runs is better off piped, and a silent switch would change how
        // every existing command behaves.
        if cfg!(windows) {
            return;
        }
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({"command": "test -t 1 && echo tty || echo pipe"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("pipe"), "{out}");
    }

    #[tokio::test]
    async fn supplied_input_reaches_a_piped_command() {
        // Answering a prompt does not require a pty — only being able to write
        // to the child and then close the pipe.
        if cfg!(windows) {
            return;
        }
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({
                    "command": "read answer; echo \"got:$answer\"",
                    "stdin": "yes\n",
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("got:yes"), "{out}");
    }

    #[test]
    fn a_missing_terminal_is_declared_above_the_output_it_changes() {
        // The branch that sets this needs a machine with no working pty, which
        // is the one thing a test here cannot arrange — so the note is tested
        // as text and the decision that reaches it is a two-arm match.
        //
        // What matters is that it comes first. A model reading `git`'s piped
        // output without knowing the terminal never arrived concludes something
        // false about the project, and a caveat underneath the output is read
        // after the conclusion has been drawn.
        let report = with_no_terminal_note(
            "cannot open a pseudo-terminal: no console host",
            "on branch main",
        );
        assert!(report.starts_with("Note:"), "{report}");
        assert!(report.contains("no console host"), "{report}");
        assert!(report.contains("on branch main"), "{report}");
    }

    #[tokio::test]
    async fn a_pty_command_reports_a_non_zero_exit_the_same_way() {
        // Both paths go through one assembler, so an exit code means the same
        // thing however the command was run.
        if cfg!(windows) {
            return;
        }
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({"command": "echo oops; exit 3", "pty": true}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().starts_with("Exit code 3"), "{out}");
        assert!(out.to_text().contains("oops"), "{out}");
    }

    #[tokio::test]
    async fn a_pty_command_still_answers_to_the_timeout() {
        // The risk this feature could add: under a pty an interactive program
        // waits rather than hitting end-of-file, so a session could hang for
        // good if the ceiling did not hold.
        if cfg!(windows) {
            return;
        }
        let (ctx, _dir) = test_ctx();
        let err = RunCommand
            .execute(
                serde_json::json!({"command": "read answer", "pty": true, "timeout_secs": 1}),
                &ctx,
            )
            .await
            .expect_err("a command waiting for input must not hang the session");
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[tokio::test]
    async fn captures_stdout() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(serde_json::json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("hello"));
    }

    #[tokio::test]
    async fn runs_in_the_workspace_by_default() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("marker.txt"), "").unwrap();
        let out = RunCommand
            .execute(serde_json::json!({"command": "ls"}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("marker.txt"));
    }

    // `shell_invocation` runs the platform's own shell -- /bin/sh elsewhere,
    // cmd.exe on Windows -- so a test that exercises shell *syntax* has to
    // speak the right dialect. The behavior under test is identical on both.

    /// Writes to stderr, then exits non-zero.
    #[cfg(windows)]
    const FAILS_WITH_STDERR: &str = "echo oops 1>&2 & exit 3";
    #[cfg(not(windows))]
    const FAILS_WITH_STDERR: &str = "echo oops >&2; exit 3";

    /// Blocks on a line of stdin, then reports that it got past the read.
    #[cfg(windows)]
    const READS_STDIN: &str = "set /p line= & echo got:";
    #[cfg(not(windows))]
    const READS_STDIN: &str = "read line; echo \"got:$line\"";

    #[tokio::test]
    async fn a_failing_command_returns_its_output_not_an_error() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(serde_json::json!({"command": FAILS_WITH_STDERR}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("Exit code 3"));
        assert!(out.to_text().contains("oops"));
    }

    #[tokio::test]
    async fn silent_success_is_reported_explicitly() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(serde_json::json!({"command": "true"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out.to_text(), "(no output)");
    }

    /// Records progress reports with the moment each one arrived.
    #[derive(Default)]
    struct Recorder {
        lines: std::sync::Mutex<Vec<(std::time::Instant, String)>>,
    }

    #[async_trait]
    impl ToolProgress for Recorder {
        async fn step(&self, label: String) {
            self.lines
                .lock()
                .unwrap()
                .push((std::time::Instant::now(), label));
        }
    }

    /// Prints, waits, then prints again — so "did the first line arrive before
    /// the command ended" is a question with an unambiguous answer.
    ///
    /// `sleep` on both, rather than `timeout /t` on Windows: `timeout` refuses
    /// to run at all when stdin is redirected, and every command here runs with
    /// stdin closed. `sleep` is already proven on the Windows runner by
    /// [`a_hanging_command_is_killed_by_the_timeout`] below.
    ///
    /// Three seconds so the margin below is two, not a tenth: CI runners are
    /// shared and a first line that is merely slow must not read as one that
    /// never streamed.
    #[cfg(windows)]
    const PRINTS_THEN_WAITS: &str = "echo first & sleep 3 & echo second";
    #[cfg(not(windows))]
    const PRINTS_THEN_WAITS: &str = "echo first; sleep 3; echo second";

    #[tokio::test]
    async fn output_reaches_the_screen_while_the_command_is_still_running() {
        let (ctx, _dir) = test_ctx();
        let recorder = Arc::new(Recorder::default());
        let ctx = ctx.with_progress(recorder.clone());

        let started = std::time::Instant::now();
        let out = RunCommand
            .execute(
                serde_json::json!({"command": PRINTS_THEN_WAITS, "timeout_secs": 30}),
                &ctx,
            )
            .await
            .unwrap();
        let finished = started.elapsed();

        // The command really did take a while, or the timing below proves
        // nothing about streaming.
        assert!(finished >= Duration::from_secs(3), "{finished:?}");

        let lines = recorder.lines.lock().unwrap();
        let first = lines
            .iter()
            .find(|(_, text)| text.contains("first"))
            .expect("the first line must have been reported");
        assert!(
            first.0.duration_since(started) < Duration::from_secs(2),
            "the first line arrived after the command ended, which is not streaming"
        );

        // And the model still gets everything, in one piece.
        assert!(out.to_text().contains("first") && out.to_text().contains("second"));
    }

    #[tokio::test]
    async fn a_command_with_no_progress_listener_still_returns_its_output() {
        // The CLI's piped mode and every test binds no progress handle. The
        // streaming path must not depend on anyone watching.
        let (ctx, _dir) = test_ctx();
        assert!(ctx.progress.is_none());
        let out = RunCommand
            .execute(serde_json::json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("hello"));
    }

    /// Reading is byte-oriented, so a build log does not end at the first byte
    /// that is not text.
    ///
    /// Unix only. `cmd` has no straightforward way to emit a raw `0xFF`, and a
    /// Windows variant that printed ordinary text would be a test that passes
    /// without exercising anything — worse than an absent one, because it reads
    /// in CI as coverage.
    #[cfg(not(windows))]
    #[tokio::test]
    async fn output_that_is_not_utf8_does_not_end_the_stream() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({"command": "printf '\\xff\\n'; echo after", "timeout_secs": 20}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            out.to_text().contains("after"),
            "reading stopped at the bad byte: {out:?}"
        );
    }

    #[tokio::test]
    async fn a_hanging_command_is_killed_by_the_timeout() {
        let (ctx, _dir) = test_ctx();
        let err = RunCommand
            .execute(
                serde_json::json!({"command": "sleep 30", "timeout_secs": 1}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn stdin_is_closed_so_interactive_reads_do_not_hang() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({"command": READS_STDIN, "timeout_secs": 5}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("got:"));
    }

    #[tokio::test]
    async fn cancellation_stops_the_command() {
        let (ctx, _dir) = test_ctx();
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let err = RunCommand
            .execute(
                serde_json::json!({"command": "sleep 30", "timeout_secs": 60}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Canceled));
    }

    #[tokio::test]
    async fn rejects_an_empty_command() {
        let (ctx, _dir) = test_ctx();
        assert!(matches!(
            RunCommand
                .execute(serde_json::json!({"command": "   "}), &ctx)
                .await,
            Err(ToolError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn cwd_outside_the_workspace_is_refused() {
        let (ctx, _dir) = test_ctx();
        let err = RunCommand
            .execute(serde_json::json!({"command": "ls", "cwd": "/etc"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::OutsideWorkspace { .. }));
    }

    /// The cut is sized to the model, not to this file.
    ///
    /// 64 KB of output is about sixteen thousand tokens: twice what an 8k local
    /// model can hold, and under two percent of a million-token one. One number
    /// could not be right for both.
    #[test]
    fn the_cut_is_sized_to_the_window_that_has_to_hold_it() {
        let (ctx, _dir) = test_ctx();
        let small = ctx.clone().with_budget(OutputBudget::for_window(8_192));
        let large = ctx.clone().with_budget(OutputBudget::for_window(1_000_000));
        let text = "x".repeat(output_cap(large.budget) * 2);

        let cut_small = for_the_model(&text, "stdout", &small);
        let cut_large = for_the_model(&text, "stdout", &large);

        assert!(
            cut_small.len() < cut_large.len(),
            "an 8k model was handed as much as a 1M one: {} vs {}",
            cut_small.len(),
            cut_large.len()
        );
        // What a small window is handed has to actually fit in it, which the
        // constant this replaced did not: 64 KB is two 8k windows.
        assert!(
            cut_small.len() / 4 < 8_192 / 2,
            "{} bytes is still more than half an 8k window",
            cut_small.len()
        );
        // And both still say that something went.
        assert!(cut_small.contains("bytes omitted"), "{cut_small}");
        assert!(cut_large.contains("bytes omitted"), "{cut_large}");
    }

    #[test]
    fn truncation_keeps_both_ends() {
        let (ctx, _dir) = test_ctx();
        let text = format!("HEAD{}TAIL", "x".repeat(output_cap(ctx.budget) * 2));
        let out = for_the_model(&text, "stdout", &ctx);
        assert!(out.starts_with("HEAD"));
        assert!(out.ends_with("TAIL"));
        assert!(out.contains("bytes omitted"));
    }

    /// Without somewhere to write it, a cut says what it always said. Every
    /// caller that runs a tool outside a session is in this case.
    #[test]
    fn with_nowhere_to_write_a_cut_only_says_how_much_it_dropped() {
        let (ctx, _dir) = test_ctx();
        let text = "x".repeat(output_cap(ctx.budget) * 2);
        let out = for_the_model(&text, "stdout", &ctx);
        assert!(out.contains("bytes omitted"), "{out}");
        assert!(!out.contains("read_file"), "{out}");
    }

    #[test]
    fn a_cut_stream_is_written_out_whole_and_named_in_the_gap() {
        let (mut ctx, _dir) = test_ctx();
        let spills = TempDir::new().unwrap();
        ctx.command_output = Some(spills.path().to_path_buf());
        ctx.session_id = Some("session-1".into());
        ctx.call_id = Some("toolu_01".into());

        let text = format!("HEAD{}TAIL", "x".repeat(output_cap(ctx.budget) * 2));
        let out = for_the_model(&text, "stdout", &ctx);

        let path = spilled_path(&out).expect("the gap names a file");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "the file is the whole stream, not the part that was shown"
        );
        // The middle is the point: it is what the cut took and the only thing
        // that could not be read any other way.
        assert!(out.contains("read_file"), "{out}");
    }

    /// The assertion the whole feature rests on. The file is outside the
    /// workspace, so a path the read guard refuses is a path the model is
    /// told to open and cannot.
    #[test]
    fn the_path_in_the_gap_is_one_the_read_guard_allows() {
        let (mut ctx, _dir) = test_ctx();
        let spills = TempDir::new().unwrap();
        let dir = spills.path().canonicalize().unwrap();
        ctx.command_output = Some(dir.clone());
        ctx.readable_roots.push(dir);

        let text = "x".repeat(output_cap(ctx.budget) * 2);
        let out = for_the_model(&text, "stdout", &ctx);
        let path = spilled_path(&out).expect("the gap names a file");

        ctx.resolve_read(&path.to_string_lossy())
            .expect("read_file must be able to open what the gap points at");
    }

    #[test]
    fn only_the_newest_spills_are_kept() {
        let dir = TempDir::new().unwrap();
        for i in 0..KEPT_SPILLS + 5 {
            let path = dir.path().join(format!("{i}.txt"));
            let file = std::fs::File::create(&path).unwrap();
            // Stamped rather than written in order: twenty-five writes can
            // land inside one tick of the filesystem's clock, and their order
            // is the whole thing under test.
            file.set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + i as u64),
            )
            .unwrap();
        }
        prune(dir.path(), KEPT_SPILLS);

        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), KEPT_SPILLS);
        assert!(!left.contains(&"0.txt".to_string()), "{left:?}");
        assert!(
            left.contains(&format!("{}.txt", KEPT_SPILLS + 4)),
            "{left:?}"
        );
    }

    /// An id a provider chose, not one this process did.
    #[test]
    fn an_id_with_path_separators_in_it_cannot_escape_the_directory() {
        assert_eq!(slug("../../etc/passwd"), "etc-passwd");
        assert_eq!(slug("toolu_01AbC"), "toolu-01AbC");
    }

    /// The whole loop, through the two real tools rather than the helpers: a
    /// command prints more than fits, and the path in the gap opens.
    #[tokio::test]
    async fn a_command_that_prints_too_much_can_be_read_back_through_read_file() {
        if cfg!(windows) {
            return;
        }
        let (mut ctx, _dir) = test_ctx();
        let spills = TempDir::new().unwrap();
        let dir = spills.path().canonicalize().unwrap();
        ctx.command_output = Some(dir.clone());
        ctx.readable_roots.push(dir);

        // Numbered, so a line found in the file can be shown not to be in the
        // report — the middle is the part that only the file has.
        let out = RunCommand
            .execute(
                // Past the 64 KB cut and inside read_file's 256 KB reach,
                // which is the case where the gap says to read the file.
                serde_json::json!({"command": "seq 1 6000 | sed 's/$/ padding padding/'"}),
                &ctx,
            )
            .await
            .unwrap();
        let out = out.to_text();

        let path = spilled_path(&out).expect("the gap names a file");
        let read = crate::builtin::fs::ReadFile
            .execute(
                serde_json::json!({"path": path.to_string_lossy(), "offset": 3_000, "limit": 1}),
                &ctx,
            )
            .await
            .expect("the path in the gap has to open");
        let read = read.to_text();

        assert!(read.contains("3000 padding"), "{read}");
        assert!(
            !out.contains("3000 padding"),
            "the middle is what the cut took; if it is still in the report this proves nothing"
        );
    }

    /// A build log is routinely megabytes. `read_file` windows around the
    /// offset it is given, so the size of the file is not what decides whether
    /// the model can reach into it.
    #[tokio::test]
    async fn a_spill_far_larger_than_one_read_is_still_read_files_to_open() {
        let (mut ctx, _dir) = test_ctx();
        let spills = TempDir::new().unwrap();
        let dir = spills.path().canonicalize().unwrap();
        ctx.command_output = Some(dir.clone());
        ctx.readable_roots.push(dir);

        // Numbered lines well past what one read answers with, so the line
        // asked for below is only reachable if the window followed the offset.
        let text: String = (1..=60_000).map(|i| format!("line {i}\n")).collect();
        assert!(text.len() > 600 * 1024, "the fixture has to be large");
        let out = for_the_model(&text, "stdout", &ctx);

        assert!(out.contains("read_file it"), "{out}");
        let path = spilled_path(&out).expect("the gap names a file");
        assert_eq!(std::fs::read_to_string(&path).unwrap().len(), text.len());

        let read = crate::builtin::fs::ReadFile
            .execute(
                serde_json::json!({"path": path.to_string_lossy(), "offset": 55_000, "limit": 1}),
                &ctx,
            )
            .await
            .expect("a spill this size still opens");
        assert!(read.to_text().contains("line 55000"), "{}", read.to_text());
    }

    /// A dev server saying the same thing four thousand times is the case a
    /// byte count reads as "a lot of output" and a reader reads as one line.
    #[tokio::test]
    async fn a_command_that_repeats_itself_comes_back_collapsed() {
        if cfg!(windows) {
            return;
        }
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({
                    "command": "for i in $(seq 1 4000); do echo 'WARN  connection retried'; done",
                }),
                &ctx,
            )
            .await
            .unwrap();
        let text = out.to_text();

        assert!(text.contains("WARN  connection retried"), "{text}");
        assert!(text.contains("repeated 3999 more times"), "{text}");
        // The saving is the point: 4000 lines went in and the model reads two.
        assert!(text.len() < 200, "{} bytes: {text}", text.len());
    }

    /// Nothing is collapsed under the threshold, so the ordinary command still
    /// reads exactly as it did.
    #[tokio::test]
    async fn a_short_command_that_repeats_itself_is_left_alone() {
        if cfg!(windows) {
            return;
        }
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({"command": "for i in 1 2 3 4 5; do echo same; done"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.to_text().trim(), "same\nsame\nsame\nsame\nsame");
    }

    /// Pulls the path back out of the sentence in the gap.
    fn spilled_path(report: &str) -> Option<PathBuf> {
        let (_, rest) = report.split_once("written out whole to ")?;
        let (path, _) = rest.split_once(" — read_file")?;
        Some(PathBuf::from(path))
    }
}
