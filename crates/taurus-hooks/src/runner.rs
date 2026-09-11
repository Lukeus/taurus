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
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use serde::Serialize;
use taurus_process::Tree;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
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
    /// Whether Stop arrived while the hooks ran.
    ///
    /// The hook running at that moment was ended, tree and all, and the ones
    /// after it were not started. Not a refusal: nothing decided against the
    /// call, the person at the keyboard ended the turn it belonged to.
    pub stopped: bool,
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
    ///
    /// `cancel` is the turn's Stop. A hook running when it fires is ended the
    /// way a timeout ends one, instead of being left to run out its limit with
    /// the turn waiting on it.
    pub async fn run(&self, payload: &HookPayload, cancel: &CancellationToken) -> Outcome {
        let mut outcome = Outcome::default();

        for (name, hook, paths) in &self.hooks {
            if hook.on != payload.event || !applies(hook, paths.as_ref(), payload) {
                continue;
            }
            if cancel.is_cancelled() {
                outcome.stopped = true;
                return outcome;
            }

            match execute(name, hook, payload, cancel).await {
                Verdict::Stopped => {
                    outcome.stopped = true;
                    return outcome;
                }
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
                // What it will actually get, which is what a list of what will
                // run is for. See `MAX_TIMEOUT_SECONDS`.
                timeout_seconds: hook.timeout().as_secs(),
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
    /// Stop arrived first, and the hook was ended before it could say.
    Stopped,
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
async fn execute(
    name: &str,
    hook: &Hook,
    payload: &HookPayload,
    cancel: &CancellationToken,
) -> Verdict {
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
    // A tree rather than a single child, so the timeout below can reach what
    // the hook started and not only the hook. A hook is usually a script, so
    // the child is `/bin/sh` or `cmd.exe` and the work is *its* child —
    // measured, every shape of script leaked the program it called when only
    // the child was killed, including the plainest one there is.
    let mut child = match Tree::spawn(command) {
        Ok(child) => child,
        Err(e) => {
            // The common case is a path typo, so the message names the program
            // rather than only the hook.
            return Verdict::Denied(format!("could not start '{}': {e}", hook.command));
        }
    };

    let stdin = child.take_stdin();
    let mut stdout = Captured::read(child.take_stdout());
    let mut stderr = Captured::read(child.take_stderr());
    let timeout = hook.timeout();

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
     * Joined rather than sequenced for the same reason both output pipes are
     * read alongside the wait: two pipes and one thread is a deadlock waiting
     * for whichever fills first.
     */
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

    /*
     * The wait borrows the child rather than owning it, and that is what lets
     * the timeout below end the tree at all. When `timeout` gives up it drops
     * this future; had the future owned the child, the child would have gone
     * with it — and with the child, the only handle on the tree there is.
     */
    let run = async {
        let ((), status) = tokio::join!(feed, child.wait());
        // The hook's answer is its exit code, and it has given it. What it
        // started may still hold its pipes — a formatter that forks a daemon —
        // and waiting for those to close ran a hook that passed out to its
        // timeout and denied it. What it printed by now is what it said.
        tokio::join!(stdout.settle(PIPE_GRACE), stderr.settle(PIPE_GRACE));
        status
    };
    // The wait is polled before the clock, so a hook that finishes exactly on
    // the deadline is finished rather than killed. Stop is polled last, for
    // the same reason: a hook that has already answered has answered.
    let finished = tokio::select! {
        biased;
        finished = tokio::time::timeout(timeout, run) => finished,
        _ = cancel.cancelled() => {
            // Ended the way a timeout ends it, and for the same reason: the
            // hook's own process is rarely the one doing the work. There is no
            // result left to carry a failed kill, so it goes to the log.
            if let Err(trouble) = child.end().await {
                warn!(hook = name, %trouble, "a hook ended by Stop may have left something running");
            }
            return Verdict::Stopped;
        }
    };
    let status = match finished {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Verdict::Denied(format!("could not be run: {e}")),
        Err(_) => {
            // A kill that could not be carried out is the user's business
            // rather than a log line: the hook is gone but what it started is
            // not, and a turn that said only "stopped" would be wrong about
            // the one thing a guard is for.
            let unfinished = match child.end().await {
                Ok(()) => String::new(),
                Err(trouble) => {
                    format!(", though what it started may still be running: {trouble}")
                }
            };
            return Verdict::Denied(format!(
                "did not finish within {}s and was stopped{unfinished}",
                timeout.as_secs()
            ));
        }
    };

    let stdout = stdout.text();
    let stderr = stderr.text();
    let code = status.code();
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

/// Everything a pipe says until it closes, or nothing if there is no pipe.
///
/// A read that fails ends the read rather than the hook: what arrived is kept,
/// and the exit code is still what decides.
/// Most of a hook's stdout or of its stderr that is kept.
///
/// What a passing hook prints reaches the model as a note, and a formatter
/// that lists every file it touched can print megabytes. Past this the pipe is
/// still read — a hook blocked on a full pipe would hang until its timeout —
/// but only counted.
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// How long a hook's output is waited for once the hook has exited.
///
/// Its pipes close when the last process holding them does, and that can be
/// something the hook started and left running. The hook itself has answered.
const PIPE_GRACE: Duration = Duration::from_millis(500);

/// A pipe read in the background, keeping the first [`MAX_OUTPUT_BYTES`].
struct Captured {
    reading: tokio::task::JoinHandle<()>,
    /// What was kept, and how many bytes past it were not.
    kept: Arc<std::sync::Mutex<(Vec<u8>, usize)>>,
}

impl Captured {
    fn read(pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>) -> Self {
        let kept = Arc::new(std::sync::Mutex::new((Vec::new(), 0usize)));
        let reading = tokio::spawn({
            let kept = kept.clone();
            async move {
                let Some(mut pipe) = pipe else {
                    return;
                };
                let mut buf = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
                    let room = MAX_OUTPUT_BYTES.saturating_sub(kept.0.len());
                    kept.0.extend_from_slice(&buf[..n.min(room)]);
                    kept.1 += n.saturating_sub(room);
                }
            }
        });
        Self { reading, kept }
    }

    /// Waits for the pipe to close, for at most `grace`.
    async fn settle(&mut self, grace: Duration) {
        let _ = tokio::time::timeout(grace, &mut self.reading).await;
    }

    /// What was kept, with a line saying how much was not.
    fn text(&self) -> String {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let mut text = String::from_utf8_lossy(&kept.0).into_owned();
        if kept.1 > 0 {
            text.push_str(&format!("\n[… {} more bytes not shown]", kept.1));
        }
        text
    }
}

/// Where a hook's own paths are resolved from, for callers building a payload.
pub fn relative<'a>(workspace: &Path, path: &'a Path) -> Option<&'a str> {
    path.strip_prefix(workspace).ok()?.to_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HookEvent, Match};
    use std::time::Duration;

    /// A hook that is a shell one-liner, written to a file so it can be run.
    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// A hook whose program is a script with this body, in the platform's own
    /// shell.
    ///
    /// Two bodies because `sh` and `cmd` share almost nothing, and every
    /// promise a hook makes — refusing in its own words, passing a note on,
    /// being stopped when it hangs, being told what it decides about — has to
    /// hold on both. On Windows the hook names the batch file itself, the way a
    /// `hooks.json` there would, rather than `cmd`: a Windows user's first hook
    /// is a `.bat`, and whether one starts at all is part of what is tested.
    fn scripted(dir: &Path, name: &str, unix: &str, windows: &str, on: HookEvent) -> Hook {
        #[cfg(unix)]
        let command = {
            let _ = windows;
            script(dir, name, unix)
        };
        #[cfg(windows)]
        let command = {
            let _ = unix;
            let path = dir.join(format!("{name}.bat"));
            let body = windows.replace('\n', "\r\n");
            std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
            path.display().to_string()
        };
        hook(&command, on)
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

    #[tokio::test]
    async fn a_hook_that_exits_two_refuses_the_call_in_its_own_words() {
        let dir = tempfile::tempdir().unwrap();
        let guard = scripted(
            dir.path(),
            "guard",
            "echo 'not on main' >&2; exit 2",
            "echo not on main 1>&2\nexit 2",
            HookEvent::PreToolUse,
        );
        let runner = HookRunner::new(vec![("guard".into(), guard)]);

        let payload = HookPayload::new(HookEvent::PreToolUse, dir.path());
        let outcome = runner.run(&payload, &CancellationToken::new()).await;

        assert!(outcome.is_denied());
        // The model has to be told why, or its next move is to try the same
        // thing again.
        let reason = outcome.denied.unwrap();
        assert!(reason.contains("not on main"), "{reason}");
        assert!(reason.contains("guard"), "{reason}");
    }

    #[tokio::test]
    async fn a_hook_that_cannot_run_refuses_rather_than_waving_the_call_through() {
        let dir = tempfile::tempdir().unwrap();
        let runner = HookRunner::new(vec![(
            "typo".into(),
            hook("/nonexistent/guard", HookEvent::PreToolUse),
        )]);

        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PreToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;

        // The whole argument for fail-closed: a guard that breaks must not
        // quietly stop guarding.
        assert!(outcome.is_denied());
        assert!(outcome.denied.unwrap().contains("/nonexistent/guard"));
    }

    #[tokio::test]
    async fn a_broken_hook_on_an_event_with_nothing_left_to_stop_only_reports() {
        let dir = tempfile::tempdir().unwrap();
        let runner = HookRunner::new(vec![(
            "typo".into(),
            hook("/nonexistent/guard", HookEvent::PostToolUse),
        )]);

        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PostToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;

        // The call already ran. Refusing it now would be a claim about the past.
        assert!(!outcome.is_denied());
        assert_eq!(outcome.notes.len(), 1);
    }

    #[tokio::test]
    async fn a_passing_hook_hands_its_output_to_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let note = scripted(
            dir.path(),
            "note",
            "echo 'formatted 2 files'",
            "echo formatted 2 files",
            HookEvent::PostToolUse,
        );
        let runner = HookRunner::new(vec![("fmt".into(), note)]);

        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PostToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;

        assert!(!outcome.is_denied());
        assert!(
            outcome.notes[0].contains("formatted 2 files"),
            "{outcome:?}"
        );
    }

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
         *
         * So the hook says when it has started, and says it is still alive
         * four seconds later — which it only gets to say if the stop missed
         * it. A marker rather than a pid, because a batch file has no way to
         * learn its own.
         */
        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("started");
        let alive = dir.path().join("alive");
        let mut slow = scripted(
            dir.path(),
            "slow",
            &format!(
                "echo x > \"{}\"; sleep 4; echo alive > \"{}\"",
                started.display(),
                alive.display()
            ),
            &format!(
                "echo x> \"{}\"\nping -n 5 127.0.0.1 >NUL && echo alive> \"{}\"",
                started.display(),
                alive.display()
            ),
            HookEvent::PreToolUse,
        );
        slow.timeout_seconds = 2;
        let runner = HookRunner::new(vec![("slow".into(), slow)]);

        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PreToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;

        assert!(outcome.is_denied());
        assert!(outcome.denied.unwrap().contains("did not finish"));
        // Or stopping it proved nothing.
        assert!(started.exists(), "the hook never started");
        // Comfortably past when it would have written, had it lived.
        for _ in 0..120 {
            assert!(
                !alive.exists(),
                "the hook is still running after the runner said it was stopped"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_leaves_something_holding_its_output_is_not_held_by_it() {
        // A formatter that forks a daemon and exits 0 has answered. Waiting on
        // the pipe its daemon still holds ran the hook out to its timeout, and
        // a hook that passed was denied for hanging.
        let dir = tempfile::tempdir().unwrap();
        let mut forks = hook(
            &script(dir.path(), "forks", "sleep 5 & echo fine"),
            HookEvent::PreToolUse,
        );
        forks.timeout_seconds = 10;
        let runner = HookRunner::new(vec![("forks".into(), forks)]);

        let started = std::time::Instant::now();
        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PreToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;

        assert!(!outcome.is_denied(), "{:?}", outcome.denied);
        assert!(
            outcome.notes.iter().any(|n| n.contains("fine")),
            "{:?}",
            outcome.notes
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "took {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_prints_a_lot_is_cut_down_to_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let loud = hook(
            &script(dir.path(), "loud", "head -c 1000000 /dev/zero | tr '\\0' x"),
            HookEvent::PostToolUse,
        );
        let runner = HookRunner::new(vec![("loud".into(), loud)]);

        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PostToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;

        let note = outcome.notes.first().expect("a note");
        assert!(note.len() < MAX_OUTPUT_BYTES + 200, "{} bytes", note.len());
        assert!(note.contains("more bytes not shown"));
    }

    #[tokio::test]
    async fn stop_ends_a_running_hook_instead_of_waiting_out_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("started");
        let alive = dir.path().join("alive");
        let mut slow = scripted(
            dir.path(),
            "slow",
            &format!(
                "echo x > '{}'; sleep 3 && echo alive > '{}'",
                started.display(),
                alive.display()
            ),
            &format!(
                "echo x> \"{}\"\nping -n 4 127.0.0.1 >NUL && echo alive> \"{}\"",
                started.display(),
                alive.display()
            ),
            HookEvent::PreToolUse,
        );
        slow.timeout_seconds = 60;
        let runner = HookRunner::new(vec![("slow".into(), slow)]);

        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let running = started.clone();
        tokio::spawn(async move {
            // Once it is certainly running, so this is Stop reaching a hook
            // and not Stop arriving before one started.
            while !running.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            stop.cancel();
        });

        let began = std::time::Instant::now();
        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PreToolUse, dir.path()),
                &cancel,
            )
            .await;

        assert!(outcome.stopped, "{outcome:?}");
        assert!(!outcome.is_denied(), "Stop is not a hook refusing the call");
        assert!(
            began.elapsed() < Duration::from_secs(3),
            "took {:?}, so the hook ran to its end",
            began.elapsed()
        );
        // Past when it would have written, had it lived.
        for _ in 0..80 {
            assert!(!alive.exists(), "the hook outlived Stop");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn a_limit_past_the_ceiling_is_brought_down_to_it() {
        let mut long = hook("guard", HookEvent::PreToolUse);
        long.timeout_seconds = 3_600;
        // Refused at load, the hook would not run at all.
        assert!(long.validate().is_ok());
        assert_eq!(
            long.timeout(),
            Duration::from_secs(crate::config::MAX_TIMEOUT_SECONDS)
        );
        let runner = HookRunner::new(vec![("long".into(), long)]);
        assert_eq!(
            runner.summaries()[0].timeout_seconds,
            crate::config::MAX_TIMEOUT_SECONDS,
            "the list must show the limit the hook actually gets"
        );
    }

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
        let mut deaf = scripted(
            dir.path(),
            "deaf",
            "sleep 20",
            "ping -n 21 127.0.0.1 >NUL",
            HookEvent::PreToolUse,
        );
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
        let outcome = runner.run(&payload, &CancellationToken::new()).await;
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
    /// `orphaned` puts a process between the two that exits as soon as it has
    /// started the grandchild, so the grandchild's parent is gone by the time
    /// the timeout fires.
    ///
    /// Two spellings because the shells have nothing in common here, and this
    /// is the one test that has to run on both: ending a tree is a process
    /// group on Unix and a Job Object on Windows, and neither code path is
    /// exercised by the other platform's.
    ///
    /// Detected by a file rather than by listing processes. `pgrep` is not on
    /// Windows, `tasklist` cannot filter on a command line, and a marker that
    /// never appears is the same evidence on both.
    fn leaves_a_grandchild(
        dir: &Path,
        started: &Path,
        alive: &Path,
        orphaned: bool,
    ) -> (String, Vec<String>) {
        #[cfg(unix)]
        {
            let grandchild = format!(
                "sh -c 'echo x > \"{}\"; sleep 8; echo alive > \"{}\"' &",
                started.display(),
                alive.display()
            );
            // A subshell that exits as soon as it has started the grandchild,
            // so its parent is gone before anything looks for it.
            let first = if orphaned {
                format!("({grandchild})")
            } else {
                grandchild
            };
            (script(dir, "outer", &format!("{first}\nsleep 30")), vec![])
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
                    // `&&`, not a new line, so the marker means exactly "ran
                    // its full eight seconds": a `ping` that is killed exits 1
                    // and never gets as far as the write, whatever order a
                    // kill ends the tree in.
                    "@echo off\r\necho x> \"{}\"\r\nping -n 9 127.0.0.1 >NUL && echo alive> \"{}\"\r\n",
                    started.display(),
                    alive.display()
                ),
            )
            .unwrap();
            let starts_it = format!("start \"\" /B cmd /C \"{}\"", inner.display());
            // Through a `cmd` of its own that exits once `start` returns, so
            // the grandchild's parent is gone before anything looks for it.
            let first = if orphaned {
                let middle = dir.join("middle.bat");
                std::fs::write(&middle, format!("@echo off\r\n{starts_it}\r\n")).unwrap();
                format!("cmd /C \"{}\"", middle.display())
            } else {
                starts_it
            };
            let outer = dir.join("outer.bat");
            std::fs::write(
                &outer,
                format!("@echo off\r\n{first}\r\nping -n 31 127.0.0.1 >NUL\r\n"),
            )
            .unwrap();
            // The hook names the shell rather than the file. Either works —
            // see `scripted` — and this is the spelling the tree tests were
            // measured with.
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
    /// missed a level of it, or never ran. Parent ids included, so a survivor
    /// whose parent is gone reads as one.
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
        a_timeout_reaches(false).await;
    }

    /// The same, where the process between the hook and the grandchild has
    /// already exited. A tree found by walking parent links down from the hook
    /// cannot reach it, because the link it would follow is gone — so this is
    /// the case that separates following links from ending a container the
    /// whole tree is inside.
    #[tokio::test]
    async fn a_timeout_reaches_a_grandchild_whose_parent_already_exited() {
        a_timeout_reaches(true).await;
    }

    async fn a_timeout_reaches(orphaned: bool) {
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
        let (command, args) = leaves_a_grandchild(dir.path(), &started, &alive, orphaned);

        let mut slow = hook(&command, HookEvent::PreToolUse);
        slow.args = args;
        // Five seconds, and every one of them is for the runner rather than
        // the hook. Two or three shells and a `start` have to get the
        // grandchild up *before* the timeout fires — on Windows that is `cmd`
        // starting `cmd`, cold. `up_before_the_kill` below says whether they
        // did, so a fixture that lost that race fails as one rather than as a
        // kill that missed.
        slow.timeout_seconds = 5;
        let slow_timeout = slow.timeout_seconds;
        let runner = HookRunner::new(vec![("slow".into(), slow)]);

        // Read while the hook is still inside its timeout. Two things at that
        // moment, not one: the tree, and — the question every failure of this
        // test has actually turned on — whether the grandchild had started
        // *yet*. Asserting `started.exists()` afterwards cannot tell a kill
        // that missed from a fixture that had not finished growing, and those
        // want opposite fixes.
        //
        // The marker first, and the listing on the blocking pool, because this
        // test has one thread and the listing is a process it waits for. Run
        // on that thread, it held the runtime for as long as PowerShell took to
        // start — seconds, on a cold Windows runner — and the hook's timeout
        // could not fire until it returned. Past eight seconds, the grandchild
        // finished before the kill began: its marker appeared, the tree after
        // the kill was empty, and the failure read as a kill that had missed.
        // That was every Windows failure of this test from 28 August to 10
        // September. Slowing the Unix listing by six seconds fails the same
        // assertion with the same empty tree.
        let watch = started.clone();
        let midway = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(3000)).await;
            let up = watch.exists();
            let listing = tokio::task::spawn_blocking(tree)
                .await
                .unwrap_or_else(|e| format!("({e})"));
            (listing, up)
        });

        let began = std::time::Instant::now();
        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PreToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;
        // Before the listing is waited for, so this is the hook's time alone.
        let took = began.elapsed();
        let (midway, up_before_the_kill) =
            midway.await.unwrap_or_else(|e| (format!("({e})"), false));
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
        // And ended in time for the marker below to mean anything. The
        // grandchild starts after `began` and writes eight seconds later, so a
        // kill not finished by then may have come after the write — and the
        // marker would blame the kill for a delay somewhere else, which is the
        // failure the comment on `midway` above describes.
        assert!(
            took < Duration::from_secs(8),
            "the hook was not stopped until {took:?} in, after the grandchild could have \
             finished on its own, so the marker below cannot say whether the kill reached it. \
             Something held up the timeout."
        );
        // The kill runs on a path where the tree was alive a moment ago, so it
        // reporting trouble is a defect and not a race. Checked here because
        // it names the cause directly, where the marker below only says that
        // *something* survived — three CI rounds went into working out which.
        assert!(
            !reason.contains("may still be running"),
            "the kill did not do its job: {reason}\
             \n== 3s in, before the timeout ==\n{midway}\
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
        // both numbers are margin rather than taste: a cold Windows runner is
        // slow to start a shell, and a marker written just after the watch
        // ended would pass a kill that missed.
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

    #[tokio::test]
    async fn a_hook_is_told_what_it_is_deciding_about() {
        let dir = tempfile::tempdir().unwrap();
        // Reads the payload off stdin and refuses with it, which is the only
        // way to assert what the hook actually received. `sort` is the `cat`
        // every Windows has: it reads to the end and prints what it read.
        let echoer = scripted(
            dir.path(),
            "echoer",
            "cat >&2; exit 2",
            "sort 1>&2\nexit 2",
            HookEvent::PreToolUse,
        );
        let runner = HookRunner::new(vec![("echoer".into(), echoer)]);

        let payload = HookPayload::new(HookEvent::PreToolUse, dir.path())
            .with_call(
                "run_command",
                serde_json::json!({"command": "git push --force"}),
                vec!["src/widget.rs".into()],
            )
            .with_session("s-1");
        let reason = runner
            .run(&payload, &CancellationToken::new())
            .await
            .denied
            .unwrap();

        assert!(reason.contains("pre_tool_use"), "{reason}");
        assert!(reason.contains("run_command"), "{reason}");
        assert!(reason.contains("git push --force"), "{reason}");
        assert!(reason.contains("src/widget.rs"), "{reason}");
    }

    #[tokio::test]
    async fn the_first_refusal_stops_the_rest_from_running() {
        let dir = tempfile::tempdir().unwrap();
        let deny = scripted(
            dir.path(),
            "a-deny",
            "exit 2",
            "exit 2",
            HookEvent::PreToolUse,
        );
        let marker = dir.path().join("ran");
        let second = scripted(
            dir.path(),
            "b-second",
            &format!("touch \"{}\"", marker.display()),
            &format!("echo x> \"{}\"", marker.display()),
            HookEvent::PreToolUse,
        );
        let runner = HookRunner::new(vec![("a-deny".into(), deny), ("b-second".into(), second)]);

        let outcome = runner
            .run(
                &HookPayload::new(HookEvent::PreToolUse, dir.path()),
                &CancellationToken::new(),
            )
            .await;

        assert!(outcome.is_denied());
        // The answer is already no; the user should not wait for three more
        // programs to agree.
        assert!(!marker.exists(), "later hooks must not run after a refusal");
    }

    #[tokio::test]
    async fn a_hook_only_runs_for_the_calls_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut guard = scripted(
            dir.path(),
            "guard",
            "exit 2",
            "exit 2",
            HookEvent::PreToolUse,
        );
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
        assert!(runner
            .run(&git, &CancellationToken::new())
            .await
            .is_denied());

        // Keyed by the leading word, the same unit an "always allow" uses —
        // approving `git` never approved `rm`, and a hook about `git` is not
        // about `rm` either.
        let other = HookPayload::new(HookEvent::PreToolUse, dir.path()).with_call(
            "run_command",
            serde_json::json!({"command": "ls -la"}),
            vec![],
        );
        assert!(!runner
            .run(&other, &CancellationToken::new())
            .await
            .is_denied());
    }

    #[tokio::test]
    async fn a_path_glob_selects_which_writes_a_hook_is_about() {
        let dir = tempfile::tempdir().unwrap();
        let mut guard = scripted(
            dir.path(),
            "guard",
            "exit 2",
            "exit 2",
            HookEvent::PreToolUse,
        );
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
        assert!(runner
            .run(&rust, &CancellationToken::new())
            .await
            .is_denied());

        let prose = HookPayload::new(HookEvent::PreToolUse, dir.path()).with_call(
            "write_file",
            serde_json::json!({"path": "README.md"}),
            vec!["README.md".into()],
        );
        assert!(!runner
            .run(&prose, &CancellationToken::new())
            .await
            .is_denied());
    }

    #[tokio::test]
    async fn no_hooks_configured_is_a_cheap_no() {
        let runner = HookRunner::default();
        assert!(runner.is_empty());
        assert!(!runner.has(HookEvent::PreToolUse));
    }
}
