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
    /// How long it may run before it is killed, in seconds.
    ///
    /// A hook runs inside a turn, so a hook that hangs is a turn that hangs.
    /// The default is short on purpose: this is a check, not a build.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
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

fn is_false(value: &bool) -> bool {
    !*value
}

impl Hook {
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
/// and most people will never write one.
pub fn load(dir: &Path) -> Result<HookConfig, String> {
    let path = config_file(dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(HookConfig::default());
    };

    // Parsed loosely first, so one bad entry costs itself rather than the file.
    // The same shape `taurus_mcp::load` uses, for the same reason: a hooks file
    // people hand-edit will have a typo in it, and losing every other hook to
    // it is the wrong response.
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        hooks: BTreeMap<String, serde_json::Value>,
    }

    let raw: Raw = serde_json::from_str(&text)
        .map_err(|e| format!("{} could not be read: {e}", path.display()))?;

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
    Ok(config)
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
}
