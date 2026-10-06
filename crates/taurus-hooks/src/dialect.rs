//! Hooks written for another agent: Claude Code, Codex, and GitHub Copilot.
//!
//! All three share one file shape, Claude Code's, and Copilot adds a second
//! of its own:
//!
//! ```json
//! {"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "…"}]}]}}
//! {"version": 1, "hooks": {"preToolUse": [{"type": "command", "bash": "…", "matcher": "bash"}]}}
//! ```
//!
//! An entry is told apart by its event's spelling: `PreToolUse` is Claude
//! Code's (Codex and Copilot read it the same way), `preToolUse` is Copilot's
//! own. Each entry that can run here becomes an ordinary [`Hook`] carrying a
//! [`Foreign`], and the runner speaks that dialect to it: the payload on
//! stdin is shaped as the hook's own agent shapes it, tool names and argument
//! names included, and a JSON answer on stdout is read for a decision.
//!
//! Three rules hold whatever the dialect says:
//!
//! - **A hook can refuse and can't permit.** `"allow"` changes nothing, and
//!   `"ask"` refuses, because a hook here has no way to ask anyone.
//! - **A hook that breaks on a tool event refuses**, as every Taurus hook
//!   does. Claude Code and Codex carry on past one; see
//!   [`crate::runner`] for why Taurus doesn't.
//! - **What can't run is named.** An event, a hook type or a field with
//!   nothing here to honor it comes back in [`Translated::unsupported`],
//!   never dropped in silence.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::config::{Hook, HookEntry, HookEvent};
use crate::runner::HookPayload;

/// Which agent's conventions a hook was written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    /// Claude Code's, which Codex and Copilot's PascalCase events also use.
    Claude,
    /// Copilot's own: camelCase events, `toolName` and `toolArgs`.
    Copilot,
}

/// What a hook from another agent needs beyond a [`Hook`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Foreign {
    pub dialect: Dialect,
    /// The event as the file names it, for `hook_event_name`.
    pub event: String,
    /// The tool matcher as written: a name, `A|B`, or a regex anchored at
    /// both ends. `None` matches every tool.
    pub matcher: Option<String>,
    /// Claude Code's `if`: a permission rule, `Bash(git push:*)` or
    /// `Edit(src/**)`, that the call must also match. Checked at
    /// translation; see [`Condition`].
    pub when: Option<String>,
    /// For a post-tool hook, whether it runs after success (`Some(true)`) or
    /// only after failure (`Some(false)`). Claude Code splits the two events;
    /// Taurus has one.
    pub outcome: Option<bool>,
    /// Set in the hook's environment, after the runner's own.
    pub env: BTreeMap<String, String>,
    /// Where it runs, relative to the workspace. The workspace when `None`.
    pub cwd: Option<String>,
}

/// One foreign hooks file, as Taurus hooks.
#[derive(Debug, Default)]
pub struct Translated {
    /// Named `<Event>#<n>`, counting from 1 in file order within the event.
    pub hooks: BTreeMap<String, HookEntry>,
    /// What wasn't translated: the event or hook as the file names it, and
    /// why.
    pub unsupported: Vec<(String, String)>,
    /// What's wrong with an entry that would otherwise run.
    pub problems: Vec<String>,
}

/// Whether a hooks file is another agent's rather than Taurus's.
///
/// By shape: Taurus's `hooks` maps a hook's name to an object, the others map
/// an event to a list.
pub fn is_foreign(file: &Value) -> bool {
    file.get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| hooks.values().any(Value::is_array))
}

/// Translates a foreign hooks file: `{"hooks": {...}}`, or the event map by
/// itself, as a manifest may hold it inline.
pub fn translate(file: &Value) -> Translated {
    let mut out = Translated::default();
    let events = match file.get("hooks").and_then(Value::as_object) {
        Some(events) => events,
        None => match file.as_object() {
            Some(events) => events,
            None => {
                out.problems
                    .push("the hooks have to be an object keyed by event".into());
                return out;
            }
        },
    };
    if file.get("disableAllHooks").and_then(Value::as_bool) == Some(true) {
        out.unsupported.push((
            "disableAllHooks".into(),
            "it's set, so none of these hooks run".into(),
        ));
        return out;
    }

    for (event, groups) in events {
        let Some((dialect, on, outcome)) = event_for(event) else {
            out.unsupported.push((
                event.clone(),
                "Taurus has no such event; it runs hooks before and after a tool call, when a \
                 prompt is sent, and when a turn ends"
                    .into(),
            ));
            continue;
        };
        let Some(groups) = groups.as_array() else {
            out.problems
                .push(format!("{event}: has to be a list of hooks"));
            continue;
        };
        let mut n = 0;
        for group in groups {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|m| !m.is_empty() && *m != "*" && *m != "**")
                .map(str::to_string);
            if let Some(matcher) = &matcher {
                // Checked here and compiled again by the runner, so a bad one
                // is a problem at load rather than a hook that never matches.
                if let Err(e) = compile_matcher(matcher) {
                    out.problems.push(format!(
                        "{event}: the matcher \"{matcher}\" isn't a usable regex: {e}"
                    ));
                    continue;
                }
            }
            // Claude Code's groups hold a list; Copilot's own entries are the
            // hook itself.
            let entries: Vec<&Value> = match group.get("hooks").and_then(Value::as_array) {
                Some(list) => list.iter().collect(),
                None => vec![group],
            };
            for entry in entries {
                n += 1;
                let name = format!("{event}#{n}");
                let when = entry.get("if").and_then(Value::as_str).map(str::to_string);
                if let Some(when) = &when {
                    if let Err(e) = Condition::parse(when) {
                        out.problems.push(format!("{name}: its \"if\" {e}"));
                        continue;
                    }
                }
                match command_for(entry) {
                    Ok((command, args)) => {
                        let mut env = BTreeMap::new();
                        if let Some(vars) = entry.get("env").and_then(Value::as_object) {
                            for (key, value) in vars {
                                if let Some(value) = value.as_str() {
                                    env.insert(key.clone(), value.to_string());
                                }
                            }
                        }
                        let hook = Hook {
                            on,
                            command,
                            args,
                            matches: None,
                            timeout_seconds: timeout_for(dialect, entry),
                            disabled: false,
                            foreign: Some(Box::new(Foreign {
                                dialect,
                                event: event.clone(),
                                matcher: matcher.clone(),
                                when,
                                outcome,
                                env,
                                cwd: entry.get("cwd").and_then(Value::as_str).map(str::to_string),
                            })),
                        };
                        out.hooks.insert(name, HookEntry::Hook(Box::new(hook)));
                    }
                    Err(Skip::Unsupported(why)) => out.unsupported.push((name, why)),
                    Err(Skip::Problem(why)) => out.problems.push(format!("{name}: {why}")),
                }
            }
        }
    }
    out
}

/// The Taurus event a foreign one is, with its dialect and, for a post-tool
/// event, which outcome it's for.
fn event_for(event: &str) -> Option<(Dialect, HookEvent, Option<bool>)> {
    use Dialect::{Claude, Copilot};
    Some(match event {
        "PreToolUse" => (Claude, HookEvent::PreToolUse, None),
        "PostToolUse" => (Claude, HookEvent::PostToolUse, Some(true)),
        "PostToolUseFailure" => (Claude, HookEvent::PostToolUse, Some(false)),
        "UserPromptSubmit" => (Claude, HookEvent::UserPromptSubmit, None),
        "Stop" => (Claude, HookEvent::Stop, None),
        "preToolUse" => (Copilot, HookEvent::PreToolUse, None),
        "postToolUse" => (Copilot, HookEvent::PostToolUse, Some(true)),
        "postToolUseFailure" => (Copilot, HookEvent::PostToolUse, Some(false)),
        "userPromptSubmitted" => (Copilot, HookEvent::UserPromptSubmit, None),
        "agentStop" => (Copilot, HookEvent::Stop, None),
        _ => return None,
    })
}

enum Skip {
    Unsupported(String),
    Problem(String),
}

/// The program and arguments an entry runs on this platform.
///
/// A foreign hook's command is a shell string, so it runs under a shell: the
/// one thing a Taurus hook never does, and the reason it's confined to here.
/// `exec` and `args` (Copilot's) run with no shell, as a Taurus hook does.
fn command_for(entry: &Value) -> Result<(String, Vec<String>), Skip> {
    let text = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match entry
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("command")
    {
        "command" => {}
        "prompt" | "agent" => {
            return Err(Skip::Unsupported(
                "it asks a model to decide, and Taurus hooks are programs".into(),
            ))
        }
        "mcp_tool" => {
            return Err(Skip::Unsupported(
                "it calls an MCP tool, and Taurus hooks are programs".into(),
            ))
        }
        "http" => {
            return Err(Skip::Unsupported(
                "it posts to a URL, and Taurus hooks are programs".into(),
            ))
        }
        other => {
            return Err(Skip::Unsupported(format!(
                "its type is \"{other}\", and Taurus runs only \"command\" hooks"
            )))
        }
    }
    let background = ["async", "asyncRewake"]
        .iter()
        .any(|key| entry.get(*key).and_then(Value::as_bool) == Some(true));
    if background {
        return Err(Skip::Unsupported(
            "it runs in the background and reports later, and Taurus runs every hook inline \
             with the call it's about"
                .into(),
        ));
    }
    if let Some(exec) = text("exec") {
        let args = entry
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        return Ok((exec, args));
    }

    #[cfg(windows)]
    {
        if let Some(script) = text("powershell") {
            return Ok((
                "powershell".into(),
                vec!["-NoProfile".into(), "-Command".into(), script],
            ));
        }
        if let Some(script) = text("commandWindows").or_else(|| text("command")) {
            return Ok(("cmd".into(), vec!["/C".into(), script]));
        }
        if text("bash").is_some() {
            return Err(Skip::Unsupported(
                "it gives only a bash command, and this is Windows".into(),
            ));
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(script) = text("bash") {
            return Ok(("bash".into(), vec!["-c".into(), script]));
        }
        if let Some(script) = text("command") {
            return Ok(("sh".into(), vec!["-c".into(), script]));
        }
        if text("powershell").is_some() || text("commandWindows").is_some() {
            return Err(Skip::Unsupported(
                "it gives only a Windows command, and this isn't Windows".into(),
            ));
        }
    }
    Err(Skip::Problem("has no command to run".into()))
}

/// Seconds, as each dialect spells and defaults them.
fn timeout_for(dialect: Dialect, entry: &Value) -> u64 {
    let (key, default) = match dialect {
        Dialect::Claude => ("timeout", 60),
        Dialect::Copilot => ("timeoutSec", 30),
    };
    entry
        .get(key)
        .and_then(Value::as_u64)
        .filter(|&s| s > 0)
        .unwrap_or(default)
}

/// A matcher as Claude Code reads one: a name or `A|B` matches exactly, and
/// anything else is a regex over the whole name.
pub fn compile_matcher(matcher: &str) -> Result<regex::Regex, regex::Error> {
    regex::Regex::new(&format!("^(?:{matcher})$"))
}

/// The names a Taurus tool goes by in a dialect, the first being the one the
/// payload carries. A matcher that matches any of them applies.
///
/// Several, because the agents disagree: Codex edits with `apply_patch`
/// where Claude Code has `Edit`, and a Claude Code hook written for
/// `MultiEdit` is about the same edits. A tool none of them has keeps its own
/// name, so `mcp__server__tool` matches as itself in every dialect.
pub fn tool_names(dialect: Dialect, tool: &str) -> Vec<&str> {
    match dialect {
        Dialect::Claude => match tool {
            "run_command" => vec!["Bash"],
            "read_file" => vec!["Read"],
            "write_file" => vec!["Write"],
            "edit_file" => vec!["Edit", "MultiEdit", "apply_patch"],
            "glob" => vec!["Glob"],
            "grep" => vec!["Grep"],
            "list_dir" => vec!["LS"],
            "fetch_url" => vec!["WebFetch"],
            "web_search" => vec!["WebSearch"],
            other => vec![other],
        },
        Dialect::Copilot => match tool {
            "run_command" => vec!["bash"],
            "read_file" | "list_dir" => vec!["view"],
            "write_file" => vec!["create"],
            "edit_file" => vec!["edit", "str_replace_editor"],
            "fetch_url" => vec!["web_fetch"],
            other => vec![other],
        },
    }
}

/// Claude Code's `if`, compiled: a tool name and, optionally, what its
/// command or path must look like, in permission-rule syntax.
#[derive(Debug)]
pub struct Condition {
    tool: String,
    spec: Option<Spec>,
}

#[derive(Debug)]
enum Spec {
    /// `Bash(git push:*)`, `Bash(git -C * commit *)`: against the command
    /// line, or any one command in it.
    Command(regex::Regex),
    /// `Edit(src/**)`: against the workspace-relative paths the call names.
    Paths(globset::GlobMatcher),
}

impl Condition {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let Some((tool, rest)) = text.split_once('(') else {
            return Ok(Self {
                tool: text.to_string(),
                spec: None,
            });
        };
        let Some(inner) = rest.strip_suffix(')') else {
            return Err(format!("\"{text}\" has no closing parenthesis"));
        };
        let spec = if tool == "Bash" {
            Spec::Command(command_rule(inner).map_err(|e| format!("\"{text}\" isn't usable: {e}"))?)
        } else {
            let glob = globset::Glob::new(inner.trim_start_matches("./"))
                .map_err(|e| format!("\"{text}\" isn't a usable path glob: {e}"))?;
            Spec::Paths(glob.compile_matcher())
        };
        Ok(Self {
            tool: tool.to_string(),
            spec: Some(spec),
        })
    }

    fn matches(&self, dialect: Dialect, payload: &HookPayload) -> bool {
        let Some(tool) = payload.tool.as_deref() else {
            return false;
        };
        if !tool_names(dialect, tool).contains(&self.tool.as_str()) {
            return false;
        }
        match &self.spec {
            None => true,
            Some(Spec::Command(rule)) => {
                let Some(line) = payload
                    .input
                    .as_ref()
                    .and_then(|i| i.get("command"))
                    .and_then(Value::as_str)
                else {
                    return false;
                };
                // `cd repo && git push` is a push. Over-matching runs a hook
                // once too often; under-matching lets past the call a guard
                // was written for.
                rule.is_match(line.trim())
                    || line
                        .split([';', '&', '|', '\n'])
                        .any(|part| rule.is_match(part.trim()))
            }
            Some(Spec::Paths(glob)) => payload.paths.iter().any(|p| glob.is_match(p)),
        }
    }
}

/// A Bash permission rule as a regex. `prefix:*` and a trailing ` *` match
/// the prefix alone or followed by more words; any other `*` matches
/// anything.
fn command_rule(rule: &str) -> Result<regex::Regex, regex::Error> {
    let (body, open) = match rule.strip_suffix(":*").or_else(|| rule.strip_suffix(" *")) {
        Some(body) => (body, true),
        None => (rule, false),
    };
    let body = body
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(".*");
    let tail = if open { r"(?:\s.*)?" } else { "" };
    regex::Regex::new(&format!(r"(?s)^{body}{tail}$"))
}

/// Whether a foreign hook applies to this call.
pub(crate) fn applies(
    foreign: &Foreign,
    matcher: Option<&regex::Regex>,
    condition: Option<&Condition>,
    payload: &HookPayload,
) -> bool {
    if let (Some(want), Some(ok)) = (foreign.outcome, payload.ok) {
        if want != ok {
            return false;
        }
    }
    if let Some(condition) = condition {
        if !condition.matches(foreign.dialect, payload) {
            return false;
        }
    }
    let Some(matcher) = matcher else {
        return true;
    };
    // A matcher on an event with no tool has nothing to narrow, as in Claude
    // Code.
    let Some(tool) = payload.tool.as_deref() else {
        return true;
    };
    tool_names(foreign.dialect, tool)
        .iter()
        .any(|name| matcher.is_match(name))
}

/// The payload as the hook's own agent would send it.
pub(crate) fn payload(foreign: &Foreign, payload: &HookPayload) -> Value {
    let cwd = payload.workspace.display().to_string();
    let tool = payload
        .tool
        .as_deref()
        .map(|t| tool_names(foreign.dialect, t)[0].to_string());
    let input = payload
        .tool
        .as_deref()
        .zip(payload.input.as_ref())
        .map(|(t, input)| arguments(foreign.dialect, t, input, &payload.workspace));
    let mut body = Map::new();
    let mut put = |key: &str, value: Value| {
        if !value.is_null() {
            body.insert(key.to_string(), value);
        }
    };
    match foreign.dialect {
        Dialect::Claude => {
            put(
                "session_id",
                json!(payload.session_id.clone().unwrap_or_default()),
            );
            put("cwd", json!(cwd));
            put("hook_event_name", json!(foreign.event));
            put("permission_mode", json!("default"));
            put("tool_name", json!(tool));
            put("tool_input", input.unwrap_or(Value::Null));
            put("prompt", json!(payload.prompt));
            if payload.event == HookEvent::Stop {
                put("stop_hook_active", json!(false));
            }
            if foreign.outcome == Some(false) {
                put("error", json!("the tool call failed"));
            }
            // Sent as null rather than left out when there's none, as Claude
            // Code and Codex send it.
            body.insert("transcript_path".into(), Value::Null);
        }
        Dialect::Copilot => {
            put(
                "sessionId",
                json!(payload.session_id.clone().unwrap_or_default()),
            );
            put("timestamp", json!(now_millis()));
            put("cwd", json!(cwd));
            put("toolName", json!(tool));
            put("toolArgs", input.unwrap_or(Value::Null));
            put("prompt", json!(payload.prompt));
            match (payload.event, payload.ok) {
                (HookEvent::PostToolUse, Some(true)) => {
                    put("toolResult", json!({"resultType": "success"}));
                }
                (HookEvent::PostToolUse, Some(false)) => {
                    put("error", json!("the tool call failed"));
                }
                (HookEvent::Stop, _) => put("stopReason", json!("end_turn")),
                _ => {}
            }
        }
    }
    Value::Object(body)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// A Taurus tool's arguments under the names the dialect's own tool uses.
///
/// Renamed, not rebuilt: a key with no counterpart is passed through as it
/// is. A hook reading `.tool_input.file_path` has to find it, and an absolute
/// path, because that's what a hook written for Claude Code was tested
/// against: a `null` there reads as nothing to object to.
fn arguments(dialect: Dialect, tool: &str, input: &Value, workspace: &Path) -> Value {
    let Some(input) = input.as_object() else {
        return input.clone();
    };
    let absolute = |value: &Value| -> Value {
        match value.as_str() {
            Some(path) => json!(workspace.join(path).display().to_string()),
            None => value.clone(),
        }
    };
    let renames: &[(&str, &str)] = match (dialect, tool) {
        (Dialect::Claude, "read_file" | "write_file" | "edit_file") => &[("path", "file_path")],
        (Dialect::Claude, "run_command") => &[("background", "run_in_background")],
        (Dialect::Claude, "grep") => &[
            ("include", "glob"),
            ("case_insensitive", "-i"),
            ("context", "-C"),
            ("limit", "head_limit"),
        ],
        (Dialect::Copilot, "write_file") => &[("content", "file_text")],
        (Dialect::Copilot, "edit_file") => &[("old_string", "old_str"), ("new_string", "new_str")],
        (Dialect::Copilot, "grep") => &[("include", "glob")],
        _ => &[],
    };
    let path_keys = ["path", "file_path"];
    let mut out = Map::new();
    for (key, value) in input {
        let key = renames
            .iter()
            .find(|(from, _)| from == key)
            .map(|(_, to)| *to)
            .unwrap_or(key);
        let value = if path_keys.contains(&key)
            && matches!(tool, "read_file" | "write_file" | "edit_file" | "list_dir")
        {
            absolute(value)
        } else {
            value.clone()
        };
        out.insert(key.to_string(), value);
    }
    if dialect == Dialect::Claude
        && tool == "grep"
        && input.get("files_only").and_then(Value::as_bool) == Some(true)
    {
        out.remove("files_only");
        out.insert("output_mode".into(), json!("files_with_matches"));
    }
    if dialect == Dialect::Claude && tool == "run_command" {
        if let Some(secs) = input.get("timeout_secs").and_then(Value::as_u64) {
            out.remove("timeout_secs");
            out.insert("timeout".into(), json!(secs * 1000));
        }
    }
    Value::Object(out)
}

/// What a foreign hook that exited 0 said, read from its stdout.
pub(crate) enum Answer {
    Passed(String),
    Denied(String),
}

/// Reads a decision from a foreign hook's stdout.
///
/// Any of the three agents' spellings, wherever each puts it: top-level
/// (`decision`, Copilot's `permissionDecision`) or under
/// `hookSpecificOutput`. Reading them all rather than only the hook's own
/// dialect costs nothing, and a hook that says "deny" in a neighbor's
/// spelling still meant it.
pub(crate) fn answer(event: HookEvent, stdout: &str) -> Answer {
    let Ok(Value::Object(top)) = serde_json::from_str::<Value>(stdout.trim()) else {
        return Answer::Passed(stdout.to_string());
    };
    let specific = top.get("hookSpecificOutput").and_then(Value::as_object);
    let field = |key: &str| -> Option<&str> {
        specific
            .and_then(|s| s.get(key))
            .or_else(|| top.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    // `systemMessage` last: it's meant for the person, but a hook that
    // refuses and explains itself only there (hookify does) has still
    // explained itself, and the model has to be told why.
    let reason = field("permissionDecisionReason")
        .or_else(|| field("reason"))
        .or_else(|| field("stopReason"))
        .or_else(|| field("systemMessage"));

    let mut notes: Vec<String> = ["additionalContext", "systemMessage"]
        .into_iter()
        .filter_map(field)
        .map(str::to_string)
        .collect();

    let refused = match field("permissionDecision") {
        Some("deny") => Some(reason.unwrap_or("it refused the call").to_string()),
        Some("ask") => Some(format!(
            "it asked for confirmation, which a Taurus hook can't ask for, so the call is refused{}",
            reason.map(|r| format!(": {r}")).unwrap_or_default()
        )),
        _ => None,
    }
    .or_else(|| {
        (field("decision") == Some("block"))
            .then(|| reason.unwrap_or("it blocked this").to_string())
    })
    .or_else(|| {
        (top.get("continue").and_then(Value::as_bool) == Some(false))
            .then(|| reason.unwrap_or("it asked to stop").to_string())
    });

    if let Some(reason) = refused {
        if event == HookEvent::Stop {
            // Claude Code reads a blocked Stop as "keep going", which Taurus
            // doesn't do. Said, so the reason it gave isn't lost.
            notes.push(format!(
                "it asked for the turn to go on, which Taurus doesn't do: {reason}"
            ));
        } else {
            return Answer::Denied(reason);
        }
    }
    Answer::Passed(notes.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(t: &Translated, name: &str) -> Hook {
        match &t.hooks[name] {
            HookEntry::Hook(hook) => (**hook).clone(),
            HookEntry::Toggle(_) => panic!("{name} is a toggle"),
        }
    }

    #[test]
    fn a_claude_file_is_told_from_a_taurus_one_by_shape() {
        assert!(is_foreign(&json!({"hooks": {"PreToolUse": []}})));
        assert!(is_foreign(
            &json!({"version": 1, "hooks": {"preToolUse": []}})
        ));
        assert!(!is_foreign(
            &json!({"hooks": {"fmt": {"on": "stop", "command": "true"}}})
        ));
    }

    #[test]
    fn a_claude_hook_becomes_a_shell_run_taurus_hook_on_the_same_event() {
        let t = translate(&json!({"hooks": {"PreToolUse": [
            {"matcher": "Bash", "hooks": [{"type": "command", "command": "./guard.sh", "timeout": 5}]}
        ]}}));
        assert!(t.unsupported.is_empty() && t.problems.is_empty(), "{t:?}");
        let h = hook(&t, "PreToolUse#1");
        assert_eq!(h.on, HookEvent::PreToolUse);
        assert_eq!(h.timeout_seconds, 5);
        #[cfg(not(windows))]
        assert_eq!(
            (h.command.as_str(), h.args.as_slice()),
            ("sh", &["-c".to_string(), "./guard.sh".into()][..])
        );
        let f = h.foreign.unwrap();
        assert_eq!(f.dialect, Dialect::Claude);
        assert_eq!(f.matcher.as_deref(), Some("Bash"));
    }

    #[test]
    fn copilot_entries_are_flat_and_camel_case() {
        let t = translate(&json!({"version": 1, "hooks": {"preToolUse": [
            {"type": "command", "bash": "./a.sh", "powershell": "./a.ps1", "timeoutSec": 7,
             "env": {"MODE": "strict"}, "cwd": "scripts"}
        ]}}));
        let h = hook(&t, "preToolUse#1");
        assert_eq!(h.timeout_seconds, 7);
        #[cfg(not(windows))]
        assert_eq!(h.command, "bash");
        let f = h.foreign.unwrap();
        assert_eq!(f.dialect, Dialect::Copilot);
        assert_eq!(f.env["MODE"], "strict");
        assert_eq!(f.cwd.as_deref(), Some("scripts"));
    }

    #[test]
    fn what_has_nothing_here_to_run_it_is_named_not_dropped() {
        let t = translate(&json!({"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "x"}]}],
            "PreToolUse": [{"hooks": [
                {"type": "prompt", "prompt": "is this safe?"},
                {"type": "command", "command": "x", "async": true}
            ]}]
        }}));
        assert!(t.hooks.is_empty(), "{t:?}");
        let all = t
            .unsupported
            .iter()
            .map(|(what, why)| format!("{what}: {why}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("SessionStart"), "{all}");
        assert!(
            all.contains("PreToolUse#1") && all.contains("model"),
            "{all}"
        );
        assert!(
            all.contains("PreToolUse#2") && all.contains("background"),
            "{all}"
        );
    }

    #[test]
    fn an_if_condition_narrows_a_hook_to_the_calls_it_names() {
        let bash = |command: &str| {
            HookPayload::new(HookEvent::PostToolUse, "/ws").with_call(
                "run_command",
                json!({"command": command}),
                vec![],
            )
        };
        let push = Condition::parse("Bash(git push:*)").unwrap();
        assert!(push.matches(Dialect::Claude, &bash("git push")));
        assert!(push.matches(Dialect::Claude, &bash("git push origin main")));
        assert!(push.matches(Dialect::Claude, &bash("cd repo && git push")));
        assert!(!push.matches(Dialect::Claude, &bash("git pushx")));
        assert!(!push.matches(Dialect::Claude, &bash("git status")));

        let commit = Condition::parse("Bash(git -C * commit *)").unwrap();
        assert!(commit.matches(Dialect::Claude, &bash("git -C sub commit -m x")));
        assert!(!commit.matches(Dialect::Claude, &bash("git commit -m x")));

        let src = Condition::parse("Edit(src/**)").unwrap();
        let edit = |path: &str| {
            HookPayload::new(HookEvent::PreToolUse, "/ws").with_call(
                "edit_file",
                json!({"path": path}),
                vec![path.into()],
            )
        };
        assert!(src.matches(Dialect::Claude, &edit("src/a.rs")));
        assert!(!src.matches(Dialect::Claude, &edit("docs/a.md")));
        // The tool has to be the one named, too.
        assert!(!push.matches(Dialect::Claude, &edit("git push")));
        assert!(Condition::parse("Bash(git push").is_err());
    }

    #[test]
    fn a_hook_that_runs_in_the_background_is_named_not_run_inline() {
        let t = translate(&json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": "review", "asyncRewake": true}
        ]}]}}));
        assert!(t.hooks.is_empty());
        assert!(
            t.unsupported[0].1.contains("background"),
            "{:?}",
            t.unsupported
        );
    }

    #[test]
    fn failure_only_post_hooks_keep_their_outcome() {
        let t =
            translate(&json!({"hooks": {"PostToolUseFailure": [{"hooks": [{"command": "x"}]}]}}));
        let h = hook(&t, "PostToolUseFailure#1");
        assert_eq!(h.on, HookEvent::PostToolUse);
        assert_eq!(h.foreign.unwrap().outcome, Some(false));
    }

    #[test]
    fn a_broken_matcher_is_a_problem_at_load() {
        let t = translate(
            &json!({"hooks": {"PreToolUse": [{"matcher": "Edit(", "hooks": [{"command": "x"}]}]}}),
        );
        assert!(t.hooks.is_empty());
        assert!(t.problems[0].contains("Edit("), "{:?}", t.problems);
    }

    fn foreign(dialect: Dialect, event: &str, matcher: Option<&str>) -> Foreign {
        Foreign {
            dialect,
            event: event.into(),
            matcher: matcher.map(str::to_string),
            when: None,
            outcome: None,
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    #[test]
    fn matchers_see_the_dialects_tool_names() {
        let call = |tool: &str| {
            HookPayload::new(HookEvent::PreToolUse, "/ws").with_call(tool, json!({}), vec![])
        };
        let edits = compile_matcher("Edit|Write").unwrap();
        let f = foreign(Dialect::Claude, "PreToolUse", Some("Edit|Write"));
        assert!(applies(&f, Some(&edits), None, &call("edit_file")));
        assert!(applies(&f, Some(&edits), None, &call("write_file")));
        assert!(!applies(&f, Some(&edits), None, &call("run_command")));
        // Anchored: `Edit` is not `MultiEditor`.
        let exact = compile_matcher("Edit").unwrap();
        assert!(!applies(&f, Some(&exact), None, &call("multi_editor")));
        // Codex's name for an edit reaches the same tool.
        let codex = compile_matcher("apply_patch").unwrap();
        assert!(applies(&f, Some(&codex), None, &call("edit_file")));
        // MCP tools match as themselves.
        let mcp = compile_matcher("mcp__plugin_ops_db__.*").unwrap();
        assert!(applies(
            &f,
            Some(&mcp),
            None,
            &call("mcp__plugin_ops_db__query")
        ));
        let copilot = foreign(Dialect::Copilot, "preToolUse", Some("bash"));
        let bash = compile_matcher("bash").unwrap();
        assert!(applies(&copilot, Some(&bash), None, &call("run_command")));
    }

    #[test]
    fn a_claude_payload_carries_claudes_names_and_an_absolute_path() {
        let p = HookPayload::new(HookEvent::PreToolUse, "/ws")
            .with_session("s-1")
            .with_call(
                "edit_file",
                json!({"path": "src/a.rs", "old_string": "a", "new_string": "b"}),
                vec!["src/a.rs".into()],
            );
        let body = payload(&foreign(Dialect::Claude, "PreToolUse", None), &p);
        assert_eq!(body["hook_event_name"], "PreToolUse");
        assert_eq!(body["tool_name"], "Edit");
        assert_eq!(body["session_id"], "s-1");
        assert_eq!(body["cwd"], "/ws");
        assert_eq!(
            body["tool_input"]["file_path"],
            Path::new("/ws").join("src/a.rs").display().to_string()
        );
        assert_eq!(body["tool_input"]["old_string"], "a");
        assert!(body.get("transcript_path").is_some());
    }

    #[test]
    fn a_copilot_payload_carries_copilots_names() {
        let p = HookPayload::new(HookEvent::PreToolUse, "/ws").with_call(
            "run_command",
            json!({"command": "git push"}),
            vec![],
        );
        let body = payload(&foreign(Dialect::Copilot, "preToolUse", None), &p);
        assert_eq!(body["toolName"], "bash");
        assert_eq!(body["toolArgs"]["command"], "git push");
        assert!(body["timestamp"].as_u64().is_some());
    }

    fn denied(a: Answer) -> Option<String> {
        match a {
            Answer::Denied(r) => Some(r),
            Answer::Passed(_) => None,
        }
    }

    #[test]
    fn a_json_deny_on_exit_zero_refuses() {
        let claude = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse",
            "permissionDecision":"deny","permissionDecisionReason":"not on main"}}"#;
        assert_eq!(
            denied(answer(HookEvent::PreToolUse, claude)).as_deref(),
            Some("not on main")
        );
        let copilot = r#"{"permissionDecision":"deny","permissionDecisionReason":"no"}"#;
        assert_eq!(
            denied(answer(HookEvent::PreToolUse, copilot)).as_deref(),
            Some("no")
        );
        // hookify's shape: the reason only in `systemMessage`.
        let hookify = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse",
            "permissionDecision":"deny"},"systemMessage":"**[block-rm]** No recursive deletes"}"#;
        assert_eq!(
            denied(answer(HookEvent::PreToolUse, hookify)).as_deref(),
            Some("**[block-rm]** No recursive deletes")
        );
        let legacy = r#"{"decision":"block","reason":"lint first"}"#;
        assert_eq!(
            denied(answer(HookEvent::UserPromptSubmit, legacy)).as_deref(),
            Some("lint first")
        );
        assert!(denied(answer(HookEvent::PreToolUse, r#"{"continue":false}"#)).is_some());
    }

    #[test]
    fn allow_permits_nothing_and_ask_refuses() {
        let allow = r#"{"hookSpecificOutput":{"permissionDecision":"allow"}}"#;
        assert!(denied(answer(HookEvent::PreToolUse, allow)).is_none());
        let ask = r#"{"permissionDecision":"ask","permissionDecisionReason":"prod"}"#;
        let reason = denied(answer(HookEvent::PreToolUse, ask)).unwrap();
        assert!(
            reason.contains("confirmation") && reason.contains("prod"),
            "{reason}"
        );
    }

    #[test]
    fn a_blocked_stop_is_said_rather_than_obeyed() {
        let block = r#"{"decision":"block","reason":"tests are red"}"#;
        match answer(HookEvent::Stop, block) {
            Answer::Passed(note) => assert!(note.contains("tests are red"), "{note}"),
            Answer::Denied(_) => panic!("a stop can't be refused"),
        }
    }

    #[test]
    fn context_reaches_the_model_and_plain_text_is_a_note() {
        let ctx = r#"{"hookSpecificOutput":{"additionalContext":"formatted 2 files"}}"#;
        match answer(HookEvent::PostToolUse, ctx) {
            Answer::Passed(note) => assert_eq!(note, "formatted 2 files"),
            Answer::Denied(_) => panic!(),
        }
        match answer(HookEvent::PostToolUse, "plain words") {
            Answer::Passed(note) => assert_eq!(note, "plain words"),
            Answer::Denied(_) => panic!(),
        }
    }
}
