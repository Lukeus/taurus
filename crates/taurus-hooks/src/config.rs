//! `hooks.json`, in both layers.
//!
//! Shaped like `mcp.json` rather than like `settings.json`, and for the same
//! reason that file is shaped the way it is: these are *named entries*, and a
//! workspace needs to be able to switch one of yours off without copying its
//! command line down into a file that would then rot. So a bare
//! `{"disabled": true}` under a name defined in a lower layer toggles it, and
//! anything else under that name replaces it.
//!
//! Not folded into `settings.json`. That file resolves field by field, which is
//! right for a theme and wrong for a set: merging two lists of hooks has no
//! single obvious answer, and every answer it could pick would surprise
//! somebody.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The file's name in either layer.
pub const FILE_NAME: &str = "hooks.json";

pub fn config_file(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// When a hook runs.
///
/// Four, deliberately. Each one is a place the harness already had a decision
/// or a boundary — there is no event here invented so the list would look
/// complete, because an event nobody can name a use for is still a promise to
/// keep working forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HookEvent {
    /// Before a tool call runs, and after the permission engine has allowed it.
    ///
    /// After, not before, and that is the whole security design: a hook here
    /// can refuse a call the user permitted, and cannot permit one the user
    /// refused. It narrows and never widens, so adding hooks to a machine can
    /// never make it more capable than it was — which is what lets a hook file
    /// be an ordinary piece of configuration rather than a second permission
    /// system to keep in step with the first.
    PreToolUse,
    /// After a tool call finishes, whether it succeeded or failed.
    ///
    /// Nothing to decide here — the call has already happened — so a hook at
    /// this point observes, and anything it prints reaches the model as a note
    /// on the result. Formatters and linters live here.
    PostToolUse,
    /// When the user sends a message, before the model sees it.
    UserPromptSubmit,
    /// When a turn finishes.
    Stop,
}

impl HookEvent {
    /// The name this event is written under in `hooks.json`.
    ///
    /// One spelling, used by the config, the listing, and the payload a hook
    /// reads — `{:?}` gives `PreToolUse` here and the file says `pre_tool_use`,
    /// and a listing that printed the first while the file wanted the second
    /// would be teaching people to write a key that does not parse.
    pub fn label(self) -> &'static str {
        match self {
            Self::PreToolUse => "pre_tool_use",
            Self::PostToolUse => "post_tool_use",
            Self::UserPromptSubmit => "user_prompt_submit",
            Self::Stop => "stop",
        }
    }

    /// Whether a hook on this event can stop what is about to happen.
    ///
    /// Only where there is still something to stop. A `post_tool_use` hook that
    /// exits 2 has refused a call that already ran, so its refusal is reported
    /// as a note rather than pretended into a block that never happened.
    pub fn can_deny(self) -> bool {
        matches!(self, Self::PreToolUse | Self::UserPromptSubmit)
    }
}

/// One layer's file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HookConfig {
    #[serde(default)]
    pub hooks: BTreeMap<String, HookEntry>,
    /// Entries that would not parse, by name, each with what is wrong.
    ///
    /// Carried rather than dropped, exactly as `mcp.json` carries them: an
    /// entry nobody can see is worse than one that says why it is not running.
    #[serde(skip)]
    pub invalid: BTreeMap<String, String>,
}

/// A hook, or a toggle for one defined in a lower layer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HookEntry {
    Hook(Box<Hook>),
    /// A bare `{"disabled": true}`. See the module docs.
    Toggle(Toggle),
}

impl HookEntry {
    /// See [`Hook::is_refusal`].
    pub fn is_refusal(&self) -> bool {
        matches!(self, Self::Hook(hook) if hook.is_refusal())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Toggle {
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct Hook {
    pub on: HookEvent,
    /// The program, run with no shell.
    ///
    /// No shell because a hook is configuration and a shell string is a second
    /// language inside it — one where the difference between a literal and an
    /// expansion is a quoting rule, and where the workspace path being
    /// interpolated is somebody else's directory name. `command` plus `args` is
    /// duller and has no such seam.
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Which calls this applies to. Absent means every call on this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Match>,
    /// How long it may run before it is killed, in seconds, up to
    /// [`MAX_TIMEOUT_SECONDS`]. Read it through [`Hook::timeout`].
    ///
    /// A hook runs inside a turn, so a hook that hangs is a turn that hangs.
    /// The default is short on purpose: this is a check, not a build.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// Set for a hook translated from another agent's format, which the
    /// runner then speaks that agent's protocol to. Never read from or
    /// written to `hooks.json`. See [`crate::dialect`].
    #[serde(skip)]
    #[ts(skip)]
    pub foreign: Option<Box<crate::dialect::Foreign>>,
}

/// Which calls a hook applies to.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct Match {
    /// Tool names, as `taurus tools` prints them. `*` matches any.
    ///
    /// A list rather than one name because the useful hooks are about a
    /// *capability* — "anything that writes" is three tools — and writing the
    /// same hook three times is how two of them come to disagree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// For `run_command`, the leading word of the command line.
    ///
    /// The same unit an "always allow" decision is keyed by, deliberately: a
    /// user who has learned that approving `git` does not approve `rm` should
    /// not have to learn a second granularity to write a hook about it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Globs, matched against every workspace-relative path the call names.
    ///
    /// Read from the tool's own declaration of what it touches, so it covers
    /// each tool's path arguments without this file having to know their
    /// names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

fn default_timeout() -> u64 {
    30
}

/// The longest a hook may run, whatever its `timeout_seconds` says.
///
/// A hook holds its tool call, and with it the turn, for as long as it runs.
/// Ten minutes is past any check worth running inline. A larger number is
/// brought down to this rather than refused, because a hook refused at load
/// does not run at all: the file asking for a longer guard would get none.
pub const MAX_TIMEOUT_SECONDS: u64 = 600;

fn is_false(value: &bool) -> bool {
    !*value
}

impl Hook {
    /// Whether this stands for a hooks file that can't be used, rather than being
    /// a hook anybody wrote. A listing leaves these out, and the file is reported
    /// as a problem instead. See [`UNREADABLE`].
    pub fn is_refusal(&self) -> bool {
        self.command == UNREADABLE
    }

    /// How long this hook may run before it is killed.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_seconds.min(MAX_TIMEOUT_SECONDS))
    }

    /// What is wrong with this entry, if anything.
    pub fn validate(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("has no `command`".into());
        }
        if self.timeout_seconds == 0 {
            return Err("has a `timeout_seconds` of 0, which would kill it before it ran".into());
        }
        if let Some(matches) = &self.matches {
            for glob in &matches.paths {
                globset::Glob::new(glob).map_err(|e| format!("has an unusable path glob: {e}"))?;
            }
        }
        Ok(())
    }
}

/// Reads one layer, keeping unparseable entries as diagnostics.
///
/// A missing file is an empty config, not an error: neither layer is required,
/// and most people will never write one. A file that's there and can't be read,
/// doesn't parse, or holds an entry that can't be used is the opposite case, see
/// [`refusing`]. So this no longer answers `Err`; the `Result` is the shape its
/// callers already handle.
pub fn load(dir: &Path) -> Result<HookConfig, String> {
    load_file(&config_file(dir))
}

/// [`load`], for a hooks file wherever it is. A plugin names its own.
pub fn load_file(path: &Path) -> Result<HookConfig, String> {
    load_file_with(path, &|what| refusal(path, what))
}

/// [`load_file`], with what a refusal says left to `word`, which is handed what's
/// wrong with the file. A plugin's way out isn't a `hooks.json`'s, since you can
/// also switch the plugin off.
pub fn load_file_with(path: &Path, word: &dyn Fn(&str) -> String) -> Result<HookConfig, String> {
    let path = path.to_path_buf();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HookConfig::default()),
        Err(e) => return Ok(refusing(word, &format!("could not be read: {e}"))),
    };

    // Parsed loosely first, so one bad entry doesn't cost the hooks beside it:
    // they go on running while the refusal below says what to fix. The same
    // shape `taurus_mcp::load` uses.
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        hooks: BTreeMap<String, serde_json::Value>,
    }

    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => return Ok(refusing(word, &format!("doesn't parse: {e}"))),
    };
    // By hand, because serde reads a list into a struct, and ignores a misspelled
    // `hooks` key. Either would be a file full of guards read as no guards.
    match value.as_object() {
        None => {
            return Ok(refusing(
                word,
                "isn't a JSON object with a `hooks` key in it",
            ))
        }
        Some(top) if !top.is_empty() && !top.contains_key("hooks") => {
            return Ok(refusing(word, "has no `hooks` key at its top level"))
        }
        Some(_) => {}
    }
    let raw: Raw = match serde_json::from_value(value) {
        Ok(raw) => raw,
        Err(e) => return Ok(refusing(word, &format!("isn't the right shape: {e}"))),
    };

    let mut config = HookConfig::default();
    for (name, value) in raw.hooks {
        // The two shapes are told apart by hand rather than by an untagged
        // enum. Untagged parses fine and *fails* uselessly: every mistake in a
        // hook comes back as "data did not match any variant", which names
        // neither the field that is missing nor the shape that was expected.
        // A hooks file is hand-written, so the message on a typo is most of
        // what this format is judged by.
        if is_toggle(&value) {
            match serde_json::from_value::<Toggle>(value) {
                Ok(toggle) => {
                    config.hooks.insert(name, HookEntry::Toggle(toggle));
                }
                Err(e) => {
                    config
                        .invalid
                        .insert(name, format!("could not be read: {e}"));
                }
            }
            continue;
        }

        match serde_json::from_value::<Hook>(value) {
            Ok(hook) => match hook.validate() {
                Ok(()) => {
                    config.hooks.insert(name, HookEntry::Hook(Box::new(hook)));
                }
                Err(reason) => {
                    config.invalid.insert(name, reason);
                }
            },
            // serde names the field: "missing field `command`". That is the
            // sentence someone can act on, so it is passed through rather than
            // wrapped in a summary of its own.
            Err(e) => {
                config
                    .invalid
                    .insert(name, format!("could not be read: {e}"));
            }
        }
    }
    if !config.invalid.is_empty() {
        let what: Vec<String> = config
            .invalid
            .iter()
            .map(|(name, reason)| format!("\"{name}\" {reason}"))
            .collect();
        add_refusals(
            &mut config,
            &word(&format!("has hooks it can't use: {}", what.join("; "))),
        );
        for reason in config.invalid.values_mut() {
            *reason = format!("{reason} (in {})", path.display());
        }
    }
    Ok(config)
}

/// What stands in for a `command` when the hooks that should have run are in a
/// file that can't be read, or can't be understood.
///
/// Parenthesized so it reads as what it is in a listing. The runner refuses on
/// it without starting anything, giving `args[0]` as the reason.
pub const UNREADABLE: &str = "(unreadable hooks file)";

/// The layer a hooks file that exists and can't be used amounts to.
///
/// Permissions, a directory in its place, bytes that aren't UTF-8, a syntax
/// error, the wrong shape. Whatever guards that file holds aren't known, so this
/// can't say which calls they'd cover, and treating it as empty would turn every
/// one of them off with nothing said. So it refuses on both events that can
/// still stop something, in line with a hook that can't run, and it's reported
/// as a problem as well.
///
/// An invalid entry gets the same refusal, on top of the hooks that did parse.
/// That entry may be the guard somebody relies on, and from here there's no
/// telling what it would have covered.
fn refusing(word: &dyn Fn(&str) -> String, what: &str) -> HookConfig {
    let reason = word(what);
    let mut config = HookConfig::default();
    add_refusals(&mut config, &reason);
    config.invalid.insert(FILE_NAME.to_string(), reason);
    config
}

/// What a refused prompt or call says. A trailing comma blocks every prompt
/// until it's fixed, so this names the file, what's wrong, and the way out.
fn refusal(path: &Path, what: &str) -> String {
    format!(
        "{} {what}. Its hooks can't run, so prompts and tool calls are refused until it's \
         fixed. Fix or remove the file; that takes effect on your next message.",
        path.display()
    )
}

/// A layer holding nothing but the two hooks that refuse, each giving `reason`.
/// For a caller that found the problem itself, like a plugin whose manifest names
/// a hooks file that isn't there.
pub fn refusing_layer(reason: &str) -> HookConfig {
    let mut config = HookConfig::default();
    add_refusals(&mut config, reason);
    config
}

/// The two hooks that do the refusing.
fn add_refusals(config: &mut HookConfig, reason: &str) {
    for on in [HookEvent::PreToolUse, HookEvent::UserPromptSubmit] {
        config.hooks.insert(
            format!("broken {FILE_NAME} ({})", on.label()),
            HookEntry::Hook(Box::new(Hook {
                on,
                command: UNREADABLE.to_string(),
                args: vec![reason.to_string()],
                matches: None,
                timeout_seconds: default_timeout(),
                disabled: false,
                foreign: None,
            })),
        );
    }
}

/// Whether an entry is a bare `{"disabled": ...}` rather than a hook.
///
/// By shape rather than by trying both parses, so `{"disabled": true}` with a
/// typo'd `commnad` beside it is read as a malformed *hook* — which is what it
/// is — instead of quietly succeeding as a toggle for something that was never
/// defined.
fn is_toggle(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == 1 && map.contains_key("disabled"))
}

/// One named hook after every layer has had its say, kept whether or not it
/// runs. What a screen that switches hooks on and off lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layered<L> {
    pub name: String,
    pub hook: Hook,
    /// The layer whose entry is the hook itself: the last one to define it.
    pub defined_in: L,
    /// The layer whose say on `disabled` is the one in effect: the defining
    /// layer's own flag, or a later toggle.
    pub decided_by: L,
    pub enabled: bool,
}

/// [`merge`]'s rules, keeping the hooks that end up off and where each one's
/// state was decided. Each layer comes with a label of the caller's choosing.
///
/// Problems aren't reported here; [`merge`] is what does, for the set that
/// runs.
pub fn layered<L: Clone>(layers: Vec<(L, HookConfig)>) -> Vec<Layered<L>> {
    let mut merged: BTreeMap<String, Layered<L>> = BTreeMap::new();
    for (label, layer) in layers {
        for (name, entry) in layer.hooks {
            match entry {
                HookEntry::Toggle(toggle) => {
                    if let Some(base) = merged.get_mut(&name) {
                        base.enabled = !toggle.disabled;
                        base.decided_by = label.clone();
                    }
                }
                HookEntry::Hook(hook) => {
                    let enabled = !hook.disabled;
                    merged.insert(
                        name.clone(),
                        Layered {
                            name,
                            hook: *hook,
                            defined_in: label.clone(),
                            decided_by: label.clone(),
                            enabled,
                        },
                    );
                }
            }
        }
    }
    merged.into_values().collect()
}

/// Layers, lowest precedence first, into the set that will actually run.
///
/// Returns the enabled hooks in name order and everything that could not be
/// used, with a reason. Name order rather than file order because two layers
/// have no shared order to preserve, and a hook set that reshuffles itself
/// between two identical turns is a diff nobody made.
pub fn merge(layers: Vec<HookConfig>) -> (Vec<(String, Hook)>, Vec<String>) {
    let mut merged: BTreeMap<String, Hook> = BTreeMap::new();
    let mut problems = Vec::new();

    for layer in layers {
        for (name, reason) in layer.invalid {
            problems.push(format!("hook '{name}' {reason}"));
        }
        for (name, entry) in layer.hooks {
            match entry {
                HookEntry::Toggle(toggle) => match merged.get_mut(&name) {
                    Some(base) => base.disabled = toggle.disabled,
                    None => problems.push(format!(
                        "hook '{name}' is toggled but never defined; give it a `command`"
                    )),
                },
                HookEntry::Hook(hook) => {
                    merged.insert(name, *hook);
                }
            }
        }
    }

    let enabled = merged
        .into_iter()
        .filter(|(_, hook)| !hook.disabled)
        .collect();
    (enabled, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) {
        std::fs::write(config_file(dir), body).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_empty_config_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().hooks.is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_reported_and_refuses() {
        // Bytes that aren't UTF-8, which fail the read on every platform and
        // for every user, root included.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(config_file(dir.path()), [0xff, 0xfe, 0x00]).unwrap();

        let (hooks, problems) = merge(vec![load(dir.path()).unwrap()]);

        assert!(
            problems.iter().any(|p| p.contains("could not be read")),
            "{problems:?}"
        );
        let events: Vec<HookEvent> = hooks.iter().map(|(_, hook)| hook.on).collect();
        assert!(events.contains(&HookEvent::PreToolUse), "{events:?}");
        assert!(events.contains(&HookEvent::UserPromptSubmit), "{events:?}");
    }

    #[test]
    fn one_bad_entry_does_not_cost_the_others() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"hooks":{
                "good": {"on":"pre_tool_use","command":"true"},
                "bad": {"on":"pre_tool_use"}
            }}"#,
        );

        let config = load(dir.path()).unwrap();
        assert!(config.hooks.contains_key("good"));
        // Kept as a diagnostic rather than dropped: a hook that silently is not
        // there is the worst outcome for a file whose entries are guards.
        assert!(config.invalid.contains_key("bad"), "{:?}", config.invalid);
    }

    #[test]
    fn a_hook_with_no_command_is_rejected_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"hooks":{"empty":{"on":"stop","command":"  "}}}"#,
        );
        let config = load(dir.path()).unwrap();
        assert!(config.invalid["empty"].contains("command"));
    }

    #[test]
    fn a_workspace_can_switch_off_a_global_hook_without_restating_it() {
        let global = HookConfig {
            hooks: [(
                "guard".to_string(),
                HookEntry::Hook(Box::new(Hook {
                    on: HookEvent::PreToolUse,
                    command: "true".into(),
                    args: vec![],
                    matches: None,
                    timeout_seconds: 30,
                    disabled: false,
                    foreign: None,
                })),
            )]
            .into(),
            invalid: BTreeMap::new(),
        };
        let workspace = HookConfig {
            hooks: [(
                "guard".to_string(),
                HookEntry::Toggle(Toggle { disabled: true }),
            )]
            .into(),
            invalid: BTreeMap::new(),
        };

        let (enabled, problems) = merge(vec![global, workspace]);
        assert!(enabled.is_empty(), "{enabled:?}");
        assert!(problems.is_empty(), "{problems:?}");
    }

    fn parsed(body: &str) -> HookConfig {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), body);
        load(dir.path()).unwrap()
    }

    #[test]
    fn a_listing_keeps_what_is_off_and_says_which_layer_decided() {
        let layers = vec![
            (
                "global",
                parsed(
                    r#"{"hooks":{
                    "guard": {"on":"pre_tool_use","command":"true"},
                    "fmt": {"on":"post_tool_use","command":"true","disabled":true},
                    "lint": {"on":"post_tool_use","command":"true"}
                }}"#,
                ),
            ),
            (
                "project",
                parsed(
                    r#"{"hooks":{
                    "guard": {"disabled": true},
                    "lint": {"on":"stop","command":"false"},
                    "ghost": {"disabled": true}
                }}"#,
                ),
            ),
        ];

        let listed = layered(layers);
        let find = |name: &str| listed.iter().find(|l| l.name == name).unwrap();

        let guard = find("guard");
        assert!(!guard.enabled);
        assert_eq!((guard.defined_in, guard.decided_by), ("global", "project"));

        let fmt = find("fmt");
        assert!(!fmt.enabled, "a hook off by its own flag is listed, off");
        assert_eq!(fmt.decided_by, "global");

        let lint = find("lint");
        assert_eq!(lint.defined_in, "project", "a later definition replaces");
        assert_eq!(lint.hook.on, HookEvent::Stop);

        // A toggle for nothing is `merge`'s to report, not a hook to list.
        assert!(listed.iter().all(|l| l.name != "ghost"));
    }

    #[test]
    fn toggling_a_hook_that_was_never_defined_says_so() {
        let orphan = HookConfig {
            hooks: [(
                "ghost".to_string(),
                HookEntry::Toggle(Toggle { disabled: true }),
            )]
            .into(),
            invalid: BTreeMap::new(),
        };
        let (_, problems) = merge(vec![orphan]);
        assert!(problems[0].contains("never defined"), "{problems:?}");
    }

    #[test]
    fn an_unusable_path_glob_is_caught_at_load_rather_than_at_the_first_call() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"hooks":{"g":{"on":"pre_tool_use","command":"true",
               "matches":{"paths":["src/**/["]}}}}"#,
        );
        assert!(load(dir.path()).unwrap().invalid.contains_key("g"));
    }

    #[test]
    fn only_the_events_with_something_left_to_stop_can_deny() {
        assert!(HookEvent::PreToolUse.can_deny());
        assert!(HookEvent::UserPromptSubmit.can_deny());
        // The call already ran. A "block" here would be a claim about the past.
        assert!(!HookEvent::PostToolUse.can_deny());
        assert!(!HookEvent::Stop.can_deny());
    }

    /// Whether both events that can still stop something would refuse, and the
    /// reason each would give.
    fn refusals(config: &HookConfig) -> Vec<String> {
        let (hooks, _) = merge(vec![config.clone()]);
        let found: Vec<(HookEvent, String)> = hooks
            .into_iter()
            .filter(|(_, hook)| hook.is_refusal())
            .map(|(_, hook)| (hook.on, hook.args[0].clone()))
            .collect();
        let events: Vec<HookEvent> = found.iter().map(|(on, _)| *on).collect();
        assert!(events.contains(&HookEvent::PreToolUse), "{events:?}");
        assert!(events.contains(&HookEvent::UserPromptSubmit), "{events:?}");
        found.into_iter().map(|(_, reason)| reason).collect()
    }

    #[test]
    fn a_file_with_a_syntax_error_refuses_and_says_where() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "{\"hooks\":{\"a\":{\"on\":\"stop\",\"command\":\"true\"},}}",
        );
        let config = load(dir.path()).unwrap();
        for reason in refusals(&config) {
            assert!(reason.contains(FILE_NAME), "{reason}");
            assert!(reason.contains("line 1"), "{reason}");
            assert!(reason.contains("Fix or remove"), "{reason}");
        }
        let (_, problems) = merge(vec![config]);
        assert!(
            problems.iter().any(|p| p.contains("doesn't parse")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_file_of_the_wrong_shape_refuses() {
        for body in [
            "[]",
            "3",
            r#"{"hooks":[]}"#,
            r#"{"hoks":{"guard":{"on":"pre_tool_use","command":"true"}}}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            write(dir.path(), body);
            refusals(&load(dir.path()).unwrap());
        }
    }

    #[test]
    fn an_empty_object_is_no_hooks_and_nothing_to_refuse() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "{}");
        let config = load(dir.path()).unwrap();
        assert!(config.hooks.is_empty() && config.invalid.is_empty());
    }

    #[test]
    fn an_invalid_entry_refuses_by_name_and_the_good_hook_stays() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{"hooks":{
                "good": {"on":"post_tool_use","command":"true"},
                "bad": {"on":"pre_tool_use"}
            }}"#,
        );
        let config = load(dir.path()).unwrap();
        assert!(config.hooks.contains_key("good"));
        for reason in refusals(&config) {
            assert!(reason.contains("\"bad\""), "{reason}");
            assert!(reason.contains("command"), "{reason}");
            assert!(reason.contains(FILE_NAME), "{reason}");
        }
    }

    /// The path as `hooksFileIn` in `src/lib/hooksFile.ts` reads it: absolute,
    /// after a space or `(in `, and followed by a space, a `)` or the end.
    fn named_for_the_button(message: &str, path: &Path) -> bool {
        let path = path.display().to_string();
        message.match_indices(&path).any(|(at, _)| {
            let before = message[..at].chars().last();
            let after = message[at + path.len()..].chars().next();
            matches!(before, None | Some(' ' | '('))
                && matches!(after, None | Some(' ' | ')' | '\n'))
        })
    }

    /// The "Open hooks.json" button in Settings and on a refused message finds
    /// the file by parsing it out of these sentences: see `hooksFileIn` in
    /// `src/lib/hooksFile.ts`. Rewording them so this fails takes the button away
    /// with nothing else to say so, so change that file with them.
    #[tokio::test]
    async fn the_refusal_and_the_problem_name_the_file_the_way_the_button_reads_it() {
        for body in [
            &b"{\"hooks\":{,}}"[..],
            b"[]",
            b"{\"hoks\":{}}",
            b"{\"hooks\":[]}",
            br#"{"hooks":{"bad":{"on":"pre_tool_use"}}}"#,
            &[0xFF, 0xFE],
        ] {
            let parent = tempfile::tempdir().unwrap();
            let dir = parent.path().join("my project").join(".taurus");
            std::fs::create_dir_all(&dir).unwrap();
            let path = config_file(&dir);
            assert!(path.ends_with(".taurus/hooks.json"));
            std::fs::write(&path, body).unwrap();

            let (hooks, problems) = merge(vec![load(&dir).unwrap()]);
            assert!(!problems.is_empty());
            for problem in &problems {
                assert!(named_for_the_button(problem, &path), "{problem}");
            }

            let refused = crate::HookRunner::new(hooks)
                .run(
                    &crate::HookPayload::new(HookEvent::UserPromptSubmit, parent.path()),
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await
                .denied
                .expect("it refuses");
            assert!(
                refused.starts_with("Refused by hook 'broken hooks.json ("),
                "{refused}"
            );
            assert!(named_for_the_button(&refused, &path), "{refused}");
        }
    }
}
