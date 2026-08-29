//! Running the configured hooks for one moment in a turn.
//!
//! # What a hook is told
//!
//! One JSON object on stdin. Reading it is optional — plenty of useful hooks
//! are `exit 1` in a shell script — but everything needed to make a decision is
//! there, so nothing has to be reconstructed from argv:
//!
//! ```json
//! {
//!   "event": "pre_tool_use",
//!   "workspace": "/Users/me/project",
//!   "session_id": "s-1a2b",
//!   "tool": "run_command",
//!   "input": {"command": "git push --force"},
//!   "paths": ["src/widget.rs"]
//! }
//! ```
//!
//! # What a hook says back
//!
//! The exit code, and that is all. Not a JSON protocol on stdout: a hook is
//! usually three lines of shell, and a format it has to *emit correctly* to be
//! obeyed is a format that will sometimes be emitted incorrectly — silently, in
//! the direction of not being obeyed.
//!
//! | Exit | Meaning |
//! | --- | --- |
//! | 0 | Fine. Anything on stdout is passed to the model as a note. |
//! | 2 | Refused. stderr, or stdout, is given to the model as the reason. |
//! | anything else | The hook did not work — see below. |
//!
//! # A hook that cannot run refuses
//!
//! A missing program, a crash, a timeout: on an event that can still stop
//! something, all of these deny.
//!
//! This is the uncomfortable choice and it is deliberate. A hook exists to make
//! a decision. One that could not make a decision has not approved anything,
//! and the alternative — treat a broken guard as a pass — is a guard that stops
//! guarding at the moment it breaks and says so only in a log. A typo in
//! `hooks.json` blocking every call is loud, immediate, and names the hook and
//! the exit code in the message the model reports back. That is recoverable in
//! seconds. Silently unguarded is not recoverable at all, because nobody knows
//! it happened.
//!
//! On `post_tool_use` and `stop` there is nothing left to stop, so a failure
//! there is reported as a note and the turn goes on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use crate::config::{Hook, HookEvent};

/// Exit code a hook uses to refuse. Everything else is either fine (0) or
/// broken.
pub const DENY: i32 = 2;

/// What the harness knows about the moment a hook is running in.
#[derive(Clone, Debug, Serialize)]
pub struct HookPayload {
    pub event: HookEvent,
    pub workspace: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The tool being called, on the two tool events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// Workspace-relative paths this call names, from the tool's own
    /// declaration of what it touches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// The user's message, on `user_prompt_submit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Whether the call succeeded, on `post_tool_use`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

impl HookPayload {
    pub fn new(event: HookEvent, workspace: impl Into<PathBuf>) -> Self {
        Self {
            event,
            workspace: workspace.into(),
            session_id: None,
            tool: None,
            input: None,
            paths: Vec::new(),
            prompt: None,
            ok: None,
        }
    }

    #[must_use]
    pub fn with_session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    #[must_use]
    pub fn with_call(
        mut self,
        tool: impl Into<String>,
        input: serde_json::Value,
        paths: Vec<String>,
    ) -> Self {
        self.tool = Some(tool.into());
        self.input = Some(input);
        self.paths = paths;
        self
    }

    #[must_use]
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    #[must_use]
    pub fn with_outcome(mut self, ok: bool) -> Self {
        self.ok = Some(ok);
        self
    }

    /// The leading word of a `run_command` call, which is what `matches.commands`
    /// is keyed by.
    fn leading_word(&self) -> Option<&str> {
        self.input
            .as_ref()?
            .get("command")?
            .as_str()?
            .split_whitespace()
            .next()
    }
}

/// What the hooks for one moment decided, together.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Why this was refused, naming the hook. `None` means nothing objected.
    ///
    /// First refusal wins and the rest are not run: the answer is already no,
    /// and running four more programs to collect four more reasons for it costs
    /// the user a wait for information they did not ask for.
    pub denied: Option<String>,
    /// What passing hooks printed, in the order they ran. Reaches the model.
    pub notes: Vec<String>,
}

impl Outcome {
    pub fn is_denied(&self) -> bool {
        self.denied.is_some()
    }
}

/// The configured hooks, ready to run.
///
/// Built once per reload and shared, like the tool registry: matching is a
/// string comparison and a glob, and the alternative is re-reading two files on
/// every tool call.
#[derive(Debug, Default)]
pub struct HookRunner {
    hooks: Vec<(String, Hook, Option<globset::GlobSet>)>,
}

impl HookRunner {
    /// Compiles the merged hook set.
    ///
    /// Globs are compiled here rather than per call, and a glob that will not
    /// compile drops that hook's path filter rather than the hook — it was
    /// validated at load, so reaching this is a bug, and a guard that quietly
    /// stops applying is worse than one that applies too widely.
    pub fn new(hooks: Vec<(String, Hook)>) -> Self {
        let hooks = hooks
            .into_iter()
            .map(|(name, hook)| {
                let set = hook.matches.as_ref().and_then(|m| compile(&m.paths));
                (name, hook, set)
            })
            .collect();
        Self { hooks }
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Whether anything at all is configured for this event.
    ///
    /// Checked before a payload is built, because building one means asking the
    /// tool what paths it touches — real work to do on every call in the
    /// overwhelmingly common case of no hooks at all.
    pub fn has(&self, event: HookEvent) -> bool {
        self.hooks.iter().any(|(_, hook, _)| hook.on == event)
    }

    /// Runs every hook that matches, in name order, and reports what they said.
    pub async fn run(&self, payload: &HookPayload) -> Outcome {
        let mut outcome = Outcome::default();

        for (name, hook, paths) in &self.hooks {
            if hook.on != payload.event || !applies(hook, paths.as_ref(), payload) {
                continue;
            }

            match execute(name, hook, payload).await {
                Verdict::Passed(note) => {
                    if !note.trim().is_empty() {
                        outcome
                            .notes
                            .push(format!("hook '{name}': {}", note.trim()));
                    }
                }
                Verdict::Denied(reason) => {
                    let reason = format!("Refused by hook '{name}': {reason}");
                    if payload.event.can_deny() {
                        outcome.denied = Some(reason);
                        // Stop here. See `Outcome::denied`.
                        return outcome;
                    }
                    // Nothing left to stop, so it becomes something to say.
                    outcome.notes.push(reason);
                }
            }
        }

        outcome
    }
}

/// One row of `taurus hooks list`, and of the Settings panel.
///
/// The command line is shown in full rather than summarized. A hook is a
/// program the user's own config asked to run inside every turn, and the whole
/// question someone has when they open this list is *what is that*.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct HookSummary {
    pub name: String,
    pub on: HookEvent,
    /// `command` and its arguments, as one line.
    pub command: String,
    /// What narrows it, in words, and `None` for a hook that applies to
    /// everything on its event.
    pub matches: Option<String>,
    pub timeout_seconds: u64,
}

impl HookRunner {
    /// Every hook that will run, in the order they would run in.
    pub fn summaries(&self) -> Vec<HookSummary> {
        self.hooks
            .iter()
            .map(|(name, hook, _)| HookSummary {
                name: name.clone(),
                on: hook.on,
                command: if hook.args.is_empty() {
                    hook.command.clone()
                } else {
                    format!("{} {}", hook.command, hook.args.join(" "))
                },
                matches: hook.matches.as_ref().and_then(describe_match),
                timeout_seconds: hook.timeout_seconds,
            })
            .collect()
    }
}

/// A `matches` block as a sentence, or `None` when it narrows nothing.
fn describe_match(matches: &crate::config::Match) -> Option<String> {
    let mut parts = Vec::new();
    if !matches.tools.is_empty() {
        parts.push(matches.tools.join(", "));
    }
    if !matches.commands.is_empty() {
        parts.push(format!("commands starting {}", matches.commands.join(", ")));
    }
    if !matches.paths.is_empty() {
        parts.push(format!("paths {}", matches.paths.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// One hook's verdict.
enum Verdict {
    Passed(String),
    Denied(String),
}

/// Whether a hook's `matches` covers this call.
///
/// An absent `matches` covers everything on its event, which is what makes a
/// `stop` hook — the one where there is nothing to match on — write as three
/// lines rather than as three lines and an empty object.
fn applies(hook: &Hook, paths: Option<&globset::GlobSet>, payload: &HookPayload) -> bool {
    let Some(matches) = &hook.matches else {
        return true;
    };

    if !matches.tools.is_empty() {
        let tool = payload.tool.as_deref().unwrap_or_default();
        if !matches.tools.iter().any(|t| t == "*" || t == tool) {
            return false;
        }
    }

    if !matches.commands.is_empty() {
        let Some(leading) = payload.leading_word() else {
            return false;
        };
        if !matches.commands.iter().any(|c| c == leading) {
            return false;
        }
    }

    if let Some(set) = paths {
        // Any path the call names is enough. A call that writes two files, one
        // of which a hook guards, is a call that hook is about.
        if !payload.paths.iter().any(|p| set.is_match(p)) {
            return false;
        }
    }

    true
}

fn compile(globs: &[String]) -> Option<globset::GlobSet> {
    if globs.is_empty() {
        return None;
    }
    let mut builder = globset::GlobSetBuilder::new();
    let mut usable = 0;
    for glob in globs {
        match globset::Glob::new(glob) {
            Ok(glob) => {
                builder.add(glob);
                usable += 1;
            }
            Err(e) => warn!(%glob, %e, "unusable hook path glob; ignoring this filter"),
        }
    }
    // Nothing compiled, so there is no filter left to apply. Returning the
    // empty set instead would be worse than useless: an empty `GlobSet`
    // matches *nothing*, so the hook would stop running at all — silently, and
    // in the one direction this crate never takes. Dropping the filter applies
    // it too widely, which is the failure the caller's comment asks for.
    if usable == 0 {
        return None;
    }
    builder.build().ok()
}

/// Starts one hook, feeds it the payload, and reads its verdict.
async fn execute(name: &str, hook: &Hook, payload: &HookPayload) -> Verdict {
    let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());

    let mut command = tokio::process::Command::new(&hook.command);
    command
        .args(&hook.args)
        // Run where the work is. A hook that checks a file has to be able to
        // name it the way every other tool in the turn does.
        .current_dir(&payload.workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TAURUS_HOOK_EVENT", payload.event.label())
        .env("TAURUS_WORKSPACE", &payload.workspace)
        // A timeout is only a timeout if it takes the process with it. Tokio
        // *detaches* a child whose future is dropped unless this is set, so
        // without it the branch below reported a hook as "stopped" and left it
        // running — a turn leaking one every time a hook hung, and the message
        // saying the opposite of what had happened.
        .kill_on_drop(true);
    if let Some(tool) = &payload.tool {
        command.env("TAURUS_TOOL", tool);
    }
    // So the timeout below can reach what the hook started, and not only the
    // hook. See `own_group`.
    own_group(&mut command);
    no_console(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            // The common case is a path typo, so the message names the program
            // rather than only the hook.
            return Verdict::Denied(format!("could not start '{}': {e}", hook.command));
        }
    };

    let stdin = child.stdin.take();
    // Read before the child is moved into the task below, which is the last
    // point this scope can ask.
    let group = child.id();
    let timeout = Duration::from_secs(hook.timeout_seconds);

    /*
     * Fed inside the timeout, and at the same time as the wait.
     *
     * A pipe holds about 64KB. The payload carries the tool's whole input, so
     * a `write_file` of anything sizeable fills it — and a hook that never
     * reads stdin then blocks this write until it exits of its own accord.
     * Writing *before* the timeout started made that wait unbounded: the
     * hook's `timeout_seconds` was skipped entirely, and a hook that should
     * have been denied for hanging came back with whatever it eventually
     * exited with. Measured at 20s against a 1s timeout, ending in a pass.
     *
     * Joined rather than sequenced for the same reason `wait_with_output`
     * reads both output pipes at once: two pipes and one thread is a deadlock
     * waiting for whichever fills first.
     */
    let run = async move {
        let feed = async move {
            if let Some(mut stdin) = stdin {
                // A hook that ignores stdin closes it, and writing to a closed
                // pipe is not an error worth failing a turn over.
                let _ = stdin.write_all(&body).await;
                let _ = stdin.shutdown().await;
                // Dropped here, which is what a hook reading to EOF is waiting
                // for.
            }
        };
        tokio::join!(feed, child.wait_with_output()).1
    };

    /*
     * Handed to a task, and the reason is the whole of the Windows half of
     * this.
     *
     * The wait owns the child, and the child is `kill_on_drop`. So whatever
     * holds that future has to still be holding it when the timeout branch
     * runs, or the hook is already dead and reaped before anything has asked
     * about its children. `timeout(..)` consumes the future it was given, so
     * that is out — and `select!` is no better, whatever an earlier version of
     * this comment claimed: it declares its futures in an inner block whose
     * value is the poll, so they are dropped before the arm bodies run at all.
     *
     * On Unix none of that shows, because a process group outlives its leader
     * and can still be signalled. On Windows there is no group: `taskkill /T`
     * walks down from the parent, and a parent that has been reaped has no
     * tree left to walk. Measured on a runner — the shell alive at 1.2s with
     * both children, and `taskkill` at 2s answering "the process 2172 not
     * found" while all three of its descendants stood there.
     *
     * A task is held by the runtime rather than by this scope, so `&mut` on
     * its handle can lose the race without the child going anywhere. It is
     * aborted after the kill, not before.
     */
    let mut task = Abandoned(tokio::spawn(run));
    let output = tokio::select! {
        // So a hook that finishes exactly on the deadline is finished rather
        // than killed.
        biased;
        joined = &mut task.0 => match joined {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Verdict::Denied(format!("could not be run: {e}")),
            // The task itself came apart, which is a bug here rather than in
            // the hook — but an event that can deny still has to deny.
            Err(e) => return Verdict::Denied(format!("could not be waited for: {e}")),
        },
        _ = tokio::time::sleep(timeout) => {
            // A kill that could not be carried out is the user's business
            // rather than a log line: the hook is gone but what it started is
            // not, and a turn that said only "stopped" would be wrong about
            // the one thing a guard is for.
            let unfinished = match kill_tree(group).await {
                Ok(()) => String::new(),
                Err(trouble) => {
                    format!(", though what it started may still be running: {trouble}")
                }
            };
            // After the kill, so the tree above was still standing while it
            // ran. This is what ends the hook itself, through `kill_on_drop`.
            task.0.abort();
            return Verdict::Denied(format!(
                "did not finish within {}s and was stopped{unfinished}",
                hook.timeout_seconds
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code();
    debug!(hook = name, ?code, "hook finished");

    match code {
        Some(0) => Verdict::Passed(stdout),
        Some(DENY) => {
            // stderr first: a script that refuses usually explains itself on
            // stderr, and one that has said nothing at all still has to give
            // the model something more actionable than an exit code.
            let reason = first_nonempty([&stderr, &stdout])
                .unwrap_or("it exited 2 without saying why")
                .to_string();
            Verdict::Denied(reason)
        }
        Some(code) => Verdict::Denied(format!(
            "exited {code}{}",
            first_nonempty([&stderr, &stdout])
                .map(|m| format!(": {m}"))
                .unwrap_or_default()
        )),
        // Killed by a signal.
        None => Verdict::Denied("was killed before it finished".into()),
    }
}

fn first_nonempty<'a>(candidates: impl IntoIterator<Item = &'a String>) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
}

/// Environment a hook is started with, beyond the inherited one.
///
/// Exposed for the docs and for tests; the runner sets these itself.
pub fn environment(payload: &HookPayload) -> BTreeMap<&'static str, String> {
    let mut env = BTreeMap::new();
    env.insert("TAURUS_HOOK_EVENT", payload.event.label().to_string());
    env.insert("TAURUS_WORKSPACE", payload.workspace.display().to_string());
    if let Some(tool) = &payload.tool {
        env.insert("TAURUS_TOOL", tool.clone());
    }
    env
}

/// Puts a hook in a process group of its own, so what it starts stops with it.
///
/// Duplicated from `taurus_tools::spawn` for the reason [`no_console`] below
/// is: this crate sits under that one, and two small functions are a smaller
/// cost than the dependency edge.
///
/// Without this the timeout reached the hook and nothing the hook ran. A hook
/// is usually a shell script, so the child is `/bin/sh` and the work is its
/// child — measured, every shape of script leaked the program it called,
/// including the plainest one there is.
fn own_group(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

/// A running hook that does not outlive the scope waiting on it.
///
/// A `JoinHandle` dropped is a task that keeps running, and this one holds the
/// child. Without the abort below, a turn cancelled while a hook was mid-flight
/// would leave the hook running with nothing left that could stop it — which is
/// the leak the tree kill exists to close, arriving through the other door.
struct Abandoned(tokio::task::JoinHandle<std::io::Result<std::process::Output>>);

impl Drop for Abandoned {
    fn drop(&mut self) {
        // Idempotent, so the timeout branch aborting explicitly — after its
        // kill, deliberately — costs nothing here.
        self.0.abort();
    }
}

/// The command that ends a process tree, per platform.
///
/// Taking the platform as an argument rather than reading `cfg!`, so the tests
/// can check both spellings from whichever machine is running them. Windows
/// code is otherwise never compiled on the machines this is developed on, and
/// a wrong argument to a kill is invisible even on Windows: the call succeeds
/// and kills nothing, which looks exactly like a tree that had already exited.
///
/// Duplicated from `taurus_tools::spawn` for the reason [`no_console`] below
/// is: this crate sits under that one, and a few small functions are a smaller
/// cost than the dependency edge.
///
/// The `--` is not decoration: without it procps reads `-123` as a second
/// signal option rather than a pid and signals its own group instead, which
/// leaves the runaway tree alive and kills the caller. See
/// `taurus_tools::spawn::kill_command` for the measurement.
///
/// `None` for a pid that does not name a tree. Negating the pid is what asks
/// for the group, so the arithmetic has to hold: `-0` is this process's own
/// group, `-1` is every process the user owns, and anything past `i32::MAX`
/// wraps into one of those two — `kill -KILL -4294967295` on Linux SIGKILLs
/// the session, measured. See `taurus_tools::spawn::kill_command`.
fn kill_command(pid: u32, windows: bool) -> Option<(&'static str, Vec<String>)> {
    if !(2..=i32::MAX as u32).contains(&pid) {
        return None;
    }
    Some(if windows {
        // Target first: `/T` ahead of `/PID` kills the named process and walks
        // no tree. See `taurus_tools::spawn::kill_command`.
        (
            "taskkill",
            vec!["/PID".into(), pid.to_string(), "/T".into(), "/F".into()],
        )
    } else {
        // `--` is load-bearing. See above.
        ("kill", vec!["-KILL".into(), "--".into(), format!("-{pid}")])
    })
}

/// Ends a hook and everything it started.
///
/// Neither platform's mechanism is reachable from `std` — a process group
/// signal on Unix, a Job Object on Windows — and both alternatives are an
/// `unsafe` call this workspace forbids or a platform-only crate for a single
/// function. So it runs the tool each platform ships for exactly this, on a
/// path where a hook has already hung.
///
/// Best-effort, and never the only kill: `kill_on_drop` still ends the hook
/// itself, so a machine missing the tool is left where it was rather than
/// worse off.
async fn kill_tree(leader: Option<u32>) -> Result<(), String> {
    let Some(pid) = leader else {
        return Ok(());
    };
    let Some((program, args)) = kill_command(pid, cfg!(windows)) else {
        return Err(format!("{pid} does not name a process tree"));
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(&args)
        .stdin(Stdio::null())
        // Captured, because a kill that ran and reached nothing says so on its
        // own output and nowhere else.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_console(&mut command);
    let out = command
        .output()
        .await
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = [&out.stderr, &out.stdout]
        .into_iter()
        .map(|s| String::from_utf8_lossy(s).trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "and said nothing".into());
    Err(format!(
        "{program} {} exited {:?}: {said}",
        args.join(" "),
        out.status.code()
    ))
}

/// `CREATE_NO_WINDOW`, so a hook does not flash a console on Windows.
///
/// The same rule `taurus_tools::spawn` applies to every other child process
/// here. Duplicated rather than depended on because this crate sits below that
/// one, and one constant is a smaller cost than the dependency edge.
fn no_console(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        // No `CommandExt` import: `tokio::process::Command` has its own
        // inherent `creation_flags` on Windows, and bringing the trait in
        // shadows nothing and warns. `taurus_tools::spawn` does the same.
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Where a hook's own paths are resolved from, for callers building a payload.
pub fn relative<'a>(workspace: &Path, path: &'a Path) -> Option<&'a str> {
    path.strip_prefix(workspace).ok()?.to_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookEvent;
    // Only the matching tests use it, and those are Unix-only.
    #[cfg(unix)]
    use crate::config::Match;

    /// A hook that is a shell one-liner, written to a file so it can be run.
    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// Whether a process still exists, without signalling it.
    #[cfg(unix)]
    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// Waits for a process to actually be gone, up to a generous ceiling.
    ///
    /// Polled rather than slept once. Delivering a signal and reaping the
    /// child is fast but not instant, and this crate's tests run alongside
    /// every other test binary in the workspace — a fixed wait tuned on an
    /// idle machine is a test that fails a few times a week on a busy one and
    /// teaches everybody to rerun rather than to read. The ceiling is long
    /// enough that reaching it means the kill genuinely did not happen.
    #[cfg(unix)]
    async fn gone(pid: &str) -> bool {
        for _ in 0..100 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    fn hook(command: &str, on: HookEvent) -> Hook {
        Hook {
            on,
            command: command.into(),
            args: vec![],
            matches: None,
            timeout_seconds: 5,
            disabled: false,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_exits_two_refuses_the_call_in_its_own_words() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "guard", "echo 'not on main' >&2; exit 2");
        let runner = HookRunner::new(vec![("guard".into(), hook(&path, HookEvent::PreToolUse))]);

        let payload = HookPayload::new(HookEvent::PreToolUse, dir.path());
        let outcome = runner.run(&payload).await;

        assert!(outcome.is_denied());
        // The model has to be told why, or its next move is to try the same
        // thing again.
        let reason = outcome.denied.unwrap();
        assert!(reason.contains("not on main"), "{reason}");
        assert!(reason.contains("guard"), "{reason}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_cannot_run_refuses_rather_than_waving_the_call_through() {
        let dir = tempfile::tempdir().unwrap();
        let runner = HookRunner::new(vec![(
            "typo".into(),
            hook("/nonexistent/guard", HookEvent::PreToolUse),
        )]);

        let outcome = runner
            .run(&HookPayload::new(HookEvent::PreToolUse, dir.path()))
            .await;

        // The whole argument for fail-closed: a guard that breaks must not
        // quietly stop guarding.
        assert!(outcome.is_denied());
        assert!(outcome.denied.unwrap().contains("/nonexistent/guard"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_broken_hook_on_an_event_with_nothing_left_to_stop_only_reports() {
        let dir = tempfile::tempdir().unwrap();
        let runner = HookRunner::new(vec![(
            "typo".into(),
            hook("/nonexistent/guard", HookEvent::PostToolUse),
        )]);

        let outcome = runner
            .run(&HookPayload::new(HookEvent::PostToolUse, dir.path()))
            .await;

        // The call already ran. Refusing it now would be a claim about the past.
        assert!(!outcome.is_denied());
        assert_eq!(outcome.notes.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_passing_hook_hands_its_output_to_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "note", "echo 'formatted 2 files'");
        let runner = HookRunner::new(vec![("fmt".into(), hook(&path, HookEvent::PostToolUse))]);

        let outcome = runner
            .run(&HookPayload::new(HookEvent::PostToolUse, dir.path()))
            .await;

        assert!(!outcome.is_denied());
        assert!(
            outcome.notes[0].contains("formatted 2 files"),
            "{outcome:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_hangs_is_stopped_rather_than_hanging_the_turn() {
        /*
         * The bug this grew to cover: this test used to assert only that the
         * *message* said the hook had been stopped, and the message was wrong.
         * Tokio detaches a child whose future is dropped unless the command
         * asked for `kill_on_drop`, so the hook was reported as stopped and
         * left running — one leaked process per hook that ever hung, and a
         * turn saying the opposite of what had happened. Nothing caught it,
         * because the only thing being checked was a string this file also
         * wrote.
         */
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let path = script(
            dir.path(),
            "slow",
            &format!("echo $$ > {}\nsleep 30", pidfile.display()),
        );
        let mut slow = hook(&path, HookEvent::PreToolUse);
        slow.timeout_seconds = 1;
        let runner = HookRunner::new(vec![("slow".into(), slow)]);

        let outcome = runner
            .run(&HookPayload::new(HookEvent::PreToolUse, dir.path()))
            .await;

        assert!(outcome.is_denied());
        assert!(outcome.denied.unwrap().contains("did not finish"));

        let pid = std::fs::read_to_string(&pidfile).expect("the hook never started");
        assert!(
            gone(pid.trim()).await,
            "the hook is still running after the runner said it was stopped"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_ignores_a_large_payload_still_hits_its_timeout() {
        /*
         * The bug this exists for, and it is the more serious of the two: the
         * payload was written to stdin *before* the timeout started. A pipe
         * holds about 64KB and the payload carries the tool's whole input, so
         * a `write_file` of any size filled it — and against a hook that never
         * reads stdin, that write blocked until the hook exited on its own.
         *
         * The timeout was therefore skipped entirely. Measured at 20s against
         * a 1s limit, and it did not merely run long: the hook exited 0 on its
         * own terms afterwards, so the call was **allowed**. That is the one
         * outcome this crate promises cannot happen — "a hook that cannot run
         * refuses" is the whole of its safety argument.
         */
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "deaf", "sleep 20");
        let mut deaf = hook(&path, HookEvent::PreToolUse);
        deaf.timeout_seconds = 1;
        let runner = HookRunner::new(vec![("deaf".into(), deaf)]);

        // Comfortably past a pipe buffer, which is what a real `write_file`
        // call carrying a file of any size looks like.
        let payload = HookPayload::new(HookEvent::PreToolUse, dir.path()).with_call(
            "write_file",
            serde_json::json!({ "content": "x".repeat(1_000_000) }),
            vec![],
        );

        let started = std::time::Instant::now();
        let outcome = runner.run(&payload).await;
        let took = started.elapsed();

        assert!(outcome.is_denied(), "a hook that never answered was obeyed");
        assert!(
            took < Duration::from_secs(5),
            "the 1s timeout took {took:?}, so stdin blocked outside it"
        );
    }

    /// A hook that leaves a grandchild behind, as command and args.
    ///
    /// It starts something that writes `marker` after a couple of seconds and
    /// then sits there itself, so the grandchild is what a timeout has to
    /// reach past the hook to kill.
    ///
    /// Two spellings because the shells have nothing in common here, and this
    /// is the one test that has to run on both: ending a tree is a process
    /// group on Unix and `taskkill /T` on Windows, and neither code path is
    /// exercised by the other platform's.
    ///
    /// Detected by a file rather than by listing processes. `pgrep` is not on
    /// Windows, `tasklist` cannot filter on a command line, and a marker that
    /// never appears is the same evidence on both.
    fn leaves_a_grandchild(dir: &Path, started: &Path, alive: &Path) -> (String, Vec<String>) {
        #[cfg(unix)]
        {
            let path = script(
                dir,
                "outer",
                &format!(
                    "sh -c 'echo x > \"{}\"; sleep 8; echo alive > \"{}\"' &\nsleep 30",
                    started.display(),
                    alive.display()
                ),
            );
            (path, vec![])
        }
        #[cfg(windows)]
        {
            // Two files rather than one, because the alternative is a `start`
            // whose argument is a quoted command containing quoted paths, and
            // batch quoting is where this test would go to die.
            let inner = dir.join("inner.bat");
            std::fs::write(
                &inner,
                format!(
                    "@echo off\r\necho x> \"{}\"\r\nping -n 9 127.0.0.1 >NUL\r\necho alive> \"{}\"\r\n",
                    started.display(),
                    alive.display()
                ),
            )
            .unwrap();
            let outer = dir.join("outer.bat");
            std::fs::write(
                &outer,
                format!(
                    "@echo off\r\nstart \"\" /B cmd /C \"{}\"\r\nping -n 31 127.0.0.1 >NUL\r\n",
                    inner.display()
                ),
            )
            .unwrap();
            // `CreateProcess` cannot run a .bat, so the hook names the shell.
            (
                "cmd".to_string(),
                vec!["/C".to_string(), outer.display().to_string()],
            )
        }
    }

    /// The tree as the OS sees it, for a failure message.
    ///
    /// "Something outlived the timeout" is not a report anybody can act on
    /// from a CI log they cannot attach a debugger to — which of the three
    /// processes survived says immediately whether the kill missed the tree,
    /// missed a level of it, or never ran. Parent ids included, because on
    /// Windows the parent link *is* the mechanism.
    fn tree() -> String {
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
            (
                "powershell",
                vec![
                    "-NoProfile",
                    "-Command",
                    "Get-CimInstance Win32_Process | \
                     Where-Object { $_.Name -match 'cmd|ping' } | \
                     Select-Object ProcessId,ParentProcessId,CommandLine | \
                     Format-Table -AutoSize | Out-String -Width 300",
                ],
            )
        } else {
            ("ps", vec!["-eo", "pid,ppid,pgid,command"])
        };
        let out = std::process::Command::new(program).args(args).output();
        let out = match out {
            Ok(out) => out,
            Err(e) => return format!("(could not run {program}: {e})"),
        };
        let listing: String = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(40)
            .collect::<Vec<_>>()
            .join("\n");
        if !listing.is_empty() {
            return listing;
        }
        /*
         * Nothing on stdout is two different things, and reading only stdout
         * conflated them for three CI rounds: an empty process list, and a
         * lister that failed. Every Windows failure of this test printed two
         * blank snapshots — which read as "the tree is gone, so the kill
         * worked" and was really "PowerShell said something and nobody
         * listened". The instrument built to answer the question was answering
         * it wrong.
         */
        let complaint = String::from_utf8_lossy(&out.stderr);
        let complaint = complaint.trim();
        if complaint.is_empty() {
            format!("(no matching processes; {program} exited {})", out.status)
        } else {
            format!("({program} could not list them: {complaint})")
        }
    }

    #[tokio::test]
    async fn a_timeout_reaches_what_the_hook_started_and_not_only_the_hook() {
        /*
         * A hook is nearly always a script, so the child is a shell and the
         * work is *its* child. Killing the child alone left that work running
         * while the turn reported the hook as stopped — measured in every
         * shape a script can be written in, including `echo x; sleep n`, which
         * is the plainest one there is.
         *
         * Asserted by a marker the grandchild writes after the hook is already
         * dead. If the tree was killed it never appears; if only the hook was,
         * it does. Proving that absence is what the wait below is for, and it
         * is why this is one of the slower tests here.
         */
        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("started");
        let alive = dir.path().join("alive");
        let (command, args) = leaves_a_grandchild(dir.path(), &started, &alive);

        let mut slow = hook(&command, HookEvent::PreToolUse);
        slow.args = args;
        // Five seconds, and every one of them is for the runner rather than
        // the hook. Two shells and a `start` have to get the grandchild up
        // *before* the timeout fires — on Windows that is `cmd` starting
        // `cmd`, cold — and then `taskkill` has to start, which is itself a
        // process and costs a good part of a second more.
        //
        // It was two, and two lost: three Windows runs in one afternoon, and
        // one of them failed with the marker appearing at 13s, which is a
        // grandchild that was never in the tree when the kill walked it rather
        // than one the kill let go. Widening the window is the fix because the
        // race is the *fixture's*, not the product's — and `raced` below is
        // what tells those two apart when it happens again.
        slow.timeout_seconds = 5;
        let slow_timeout = slow.timeout_seconds;
        let runner = HookRunner::new(vec![("slow".into(), slow)]);

        // Read while the hook is still inside its timeout, because the kill
        // reported the hook's own shell already gone and the question that
        // answers is whether it ever lived that long.
        // Read while the hook is still inside its timeout. Two things at that
        // moment, not one: the tree, and — the question every failure of this
        // test has actually turned on — whether the grandchild had started
        // *yet*. Asserting `started.exists()` afterwards cannot tell a kill
        // that missed from a fixture that had not finished growing, and those
        // want opposite fixes.
        let watch = started.clone();
        let midway = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(3000)).await;
            (tree(), watch.exists())
        });

        let began = std::time::Instant::now();
        let outcome = runner
            .run(&HookPayload::new(HookEvent::PreToolUse, dir.path()))
            .await;
        let (midway, up_before_the_kill) =
            midway.await.unwrap_or_else(|e| (format!("({e})"), false));
        let took = began.elapsed();
        // The moment that matters: what the kill left standing, read before
        // anything has had time to exit on its own. The tree at *failure* time
        // cannot tell a process that was spared from one that finished.
        let after_the_kill = tree();

        /*
         * The *timeout* has to be what ended this, not merely something.
         *
         * `is_denied` alone was too weak to carry the test: a hook that fell
         * over on its own is denied too, and a shell that exited early would
         * have satisfied it while killing nothing — leaving the survivor below
         * to be blamed on the tree kill. So the reason is read, and the clock
         * is checked against it.
         */
        let reason = outcome.denied.clone().unwrap_or_default();
        assert!(
            reason.contains("did not finish within"),
            "ended for some other reason than its timeout: {reason:?} after {took:?}\n{}",
            tree()
        );
        // The kill runs on a path where the tree was alive a moment ago, so it
        // reporting trouble is a defect and not a race. Checked here because
        // it names the cause directly, where the marker below only says that
        // *something* survived — three CI rounds went into working out which.
        assert!(
            !reason.contains("may still be running"),
            "the kill did not do its job: {reason}\
             \n== 1.2s in, before the timeout ==\n{midway}\
             \n== a moment after the kill ==\n{after_the_kill}"
        );
        // Asserted rather than assumed: a test that stopped something before it
        // had started would pass against the bug it was written for, which is
        // how the sibling test in `taurus_tools::jobs` first passed.
        assert!(started.exists(), "the grandchild never started");
        // And that it was there *in time*, which is the stronger claim and the
        // one the survivor below depends on. A grandchild that appeared after
        // the kill had already walked the tree was never a candidate to be
        // killed, so its survival says nothing about the kill — and a failure
        // reading "the kill did not do its job" would be blaming the wrong
        // thing. Widen the timeout above if this is what fires.
        assert!(
            up_before_the_kill,
            "the fixture lost its own race: the grandchild was not up 3s in, so the kill \
             at {}s had nothing to find. This is the timeout being too tight, not the tree \
             kill failing.\n== 3s in ==\n{midway}",
            slow_timeout
        );

        // Comfortably past when the grandchild would have written, had it
        // lived. Polled rather than slept in one go so a failure is quick.
        //
        // The grandchild waits eight seconds and this watches for twelve, and
        // both numbers are margin rather than taste. `taskkill` is a *process*
        // — starting it on a cold Windows runner costs a good part of a second
        // on its own, and at two seconds the kill was landing after the marker
        // had already been written. Measured: this test, and only on Windows.
        for _ in 0..240 {
            assert!(
                !alive.exists(),
                "the hook's own child outlived the timeout and kept working\
                 \n== a moment after the kill ==\n{after_the_kill}\
                 \n== now, {:?} in ==\n{}",
                began.elapsed(),
                tree()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn the_kill_is_spelled_the_way_each_platform_spells_it() {
        // Both arms from whichever machine is running this, because neither is
        // compiled on the other one — and a wrong argument here is invisible
        // even on the platform it is wrong for: the kill still succeeds and
        // simply reaches nothing.
        assert_eq!(
            kill_command(4321, false),
            Some((
                "kill",
                vec!["-KILL".to_string(), "--".to_string(), "-4321".to_string()]
            ))
        );
        assert_eq!(
            kill_command(4321, true),
            Some((
                "taskkill",
                vec![
                    "/PID".to_string(),
                    "4321".to_string(),
                    "/T".to_string(),
                    "/F".to_string()
                ]
            ))
        );

        // Asserted rather than exercised: `-0` is this process's own group and
        // `-1` is every process the user owns, so a test that proved the guard
        // by calling it would be the outage.
        for platform in [false, true] {
            assert!(kill_command(0, platform).is_none());
            assert!(kill_command(1, platform).is_none());
            assert!(kill_command(u32::MAX, platform).is_none());
        }
    }

    #[test]
    fn a_path_filter_that_cannot_compile_is_dropped_rather_than_silencing_the_hook() {
        /*
         * `compile` used to hand back the empty `GlobSet` it had built, and an
         * empty set matches *nothing* — so a hook whose globs all failed
         * stopped running at all, quietly, which is the direction this crate
         * never takes. Load-time validation means no real config reaches here,
         * but the fallback has to fail the way its own comment says it does.
         */
        assert!(compile(&["[".to_string()]).is_none());
        // And one bad glob among good ones keeps the good ones.
        let set = compile(&["[".to_string(), "src/**".to_string()]).expect("no filter compiled");
        assert!(set.is_match("src/widget.rs"));
        assert!(!set.is_match("docs/readme.md"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_is_told_what_it_is_deciding_about() {
        let dir = tempfile::tempdir().unwrap();
        // Reads the payload off stdin and refuses with it, which is the only
        // way to assert what the hook actually received.
        let path = script(dir.path(), "echoer", "cat >&2; exit 2");
        let runner = HookRunner::new(vec![("echoer".into(), hook(&path, HookEvent::PreToolUse))]);

        let payload = HookPayload::new(HookEvent::PreToolUse, dir.path())
            .with_call(
                "run_command",
                serde_json::json!({"command": "git push --force"}),
                vec!["src/widget.rs".into()],
            )
            .with_session("s-1");
        let reason = runner.run(&payload).await.denied.unwrap();

        assert!(reason.contains("pre_tool_use"), "{reason}");
        assert!(reason.contains("run_command"), "{reason}");
        assert!(reason.contains("git push --force"), "{reason}");
        assert!(reason.contains("src/widget.rs"), "{reason}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_first_refusal_stops_the_rest_from_running() {
        let dir = tempfile::tempdir().unwrap();
        let deny = script(dir.path(), "a-deny", "exit 2");
        let marker = dir.path().join("ran");
        let second = script(
            dir.path(),
            "b-second",
            &format!("touch {}", marker.display()),
        );
        let runner = HookRunner::new(vec![
            ("a-deny".into(), hook(&deny, HookEvent::PreToolUse)),
            ("b-second".into(), hook(&second, HookEvent::PreToolUse)),
        ]);

        let outcome = runner
            .run(&HookPayload::new(HookEvent::PreToolUse, dir.path()))
            .await;

        assert!(outcome.is_denied());
        // The answer is already no; the user should not wait for three more
        // programs to agree.
        assert!(!marker.exists(), "later hooks must not run after a refusal");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_only_runs_for_the_calls_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "guard", "exit 2");
        let mut guard = hook(&path, HookEvent::PreToolUse);
        guard.matches = Some(Match {
            commands: vec!["git".into()],
            ..Default::default()
        });
        let runner = HookRunner::new(vec![("guard".into(), guard)]);

        let git = HookPayload::new(HookEvent::PreToolUse, dir.path()).with_call(
            "run_command",
            serde_json::json!({"command": "git push"}),
            vec![],
        );
        assert!(runner.run(&git).await.is_denied());

        // Keyed by the leading word, the same unit an "always allow" uses —
        // approving `git` never approved `rm`, and a hook about `git` is not
        // about `rm` either.
        let other = HookPayload::new(HookEvent::PreToolUse, dir.path()).with_call(
            "run_command",
            serde_json::json!({"command": "ls -la"}),
            vec![],
        );
        assert!(!runner.run(&other).await.is_denied());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_path_glob_selects_which_writes_a_hook_is_about() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "guard", "exit 2");
        let mut guard = hook(&path, HookEvent::PreToolUse);
        guard.matches = Some(Match {
            paths: vec!["**/*.rs".into()],
            ..Default::default()
        });
        let runner = HookRunner::new(vec![("guard".into(), guard)]);

        let rust = HookPayload::new(HookEvent::PreToolUse, dir.path()).with_call(
            "write_file",
            serde_json::json!({"path": "src/widget.rs"}),
            vec!["src/widget.rs".into()],
        );
        assert!(runner.run(&rust).await.is_denied());

        let prose = HookPayload::new(HookEvent::PreToolUse, dir.path()).with_call(
            "write_file",
            serde_json::json!({"path": "README.md"}),
            vec!["README.md".into()],
        );
        assert!(!runner.run(&prose).await.is_denied());
    }

    #[tokio::test]
    async fn no_hooks_configured_is_a_cheap_no() {
        let runner = HookRunner::default();
        assert!(runner.is_empty());
        assert!(!runner.has(HookEvent::PreToolUse));
    }
}
