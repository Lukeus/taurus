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
use taurus_process::Tree;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    let stdout = child.take_stdout();
    let stderr = child.take_stderr();
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
        let (_, stdout, stderr, status) =
            tokio::join!(feed, read_all(stdout), read_all(stderr), child.wait());
        status.map(|status| (status, stdout, stderr))
    };
    // The wait is polled before the clock, so a hook that finishes exactly on
    // the deadline is finished rather than killed.
    let finished = tokio::time::timeout(timeout, run).await;
    let (status, stdout, stderr) = match finished {
        Ok(Ok(done)) => done,
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
                hook.timeout_seconds
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
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
async fn read_all(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes).await;
    }
    bytes
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
            .run(&HookPayload::new(HookEvent::PreToolUse, dir.path()))
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
