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
    for glob in globs {
        match globset::Glob::new(glob) {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(e) => warn!(%glob, %e, "unusable hook path glob; ignoring this filter"),
        }
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
        .env("TAURUS_WORKSPACE", &payload.workspace);
    if let Some(tool) = &payload.tool {
        command.env("TAURUS_TOOL", tool);
    }
    no_console(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            // The common case is a path typo, so the message names the program
            // rather than only the hook.
            return Verdict::Denied(format!("could not start '{}': {e}", hook.command));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        // A hook that ignores stdin closes it, and writing to a closed pipe is
        // not an error worth failing a turn over.
        let _ = stdin.write_all(&body).await;
        let _ = stdin.shutdown().await;
    }

    let timeout = Duration::from_secs(hook.timeout_seconds);
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Verdict::Denied(format!("could not be run: {e}")),
        Err(_) => {
            return Verdict::Denied(format!(
                "did not finish within {}s and was stopped",
                hook.timeout_seconds
            ))
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
    use crate::config::{HookEvent, Match};

    /// A hook that is a shell one-liner, written to a file so it can be run.
    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
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
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "slow", "sleep 30");
        let mut slow = hook(&path, HookEvent::PreToolUse);
        slow.timeout_seconds = 1;
        let runner = HookRunner::new(vec![("slow".into(), slow)]);

        let outcome = runner
            .run(&HookPayload::new(HookEvent::PreToolUse, dir.path()))
            .await;

        assert!(outcome.is_denied());
        assert!(outcome.denied.unwrap().contains("did not finish"));
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
