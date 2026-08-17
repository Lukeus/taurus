//! What the MCP panel sees, and what it is allowed to send back.
//!
//! A server on disk is a small JSON object; a server on screen is that object
//! plus three things the file does not hold — which layer defines it, whether it
//! connected, and whether the program it names can be found at all. Assembling
//! those here rather than in the frontend keeps the panel from having to
//! reimplement layering, and keeps one answer to "is this working" rather than
//! two that can disagree.
//!
//! # Secrets never make the round trip
//!
//! An `env` or `headers` value is either a `${VAR}` reference, which is a
//! variable name and safe to show, or a literal, which is usually a token. The
//! literal is not sent: the panel is told the key is set and nothing more. A
//! save then sends back the same shape, and a value marked `secret` with nothing
//! typed into it means "leave what is there" — see [`merge_secrets`]. Round-
//! tripping the token instead would put it in the IPC payload, in the frontend's
//! memory, and in any devtools session open at the time, to no purpose: the
//! panel never needs to read a credential to edit the entry it sits in.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use taurus_mcp::{ServerConfig, ServerStatus};

use crate::config::Scope;

/// Which layer defined each server, by name.
pub type LayerOf = BTreeMap<String, Scope>;

/// How a server is reached. The tagged form of the untagged config enum, so the
/// frontend can switch on one field instead of guessing from which are present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum McpTransport {
    Stdio,
    Http,
}

/// One `env` or `headers` entry, with the value withheld when it is a secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpValue {
    pub key: String,
    /// The `${VAR}` reference, or empty when [`Self::secret`] is set.
    pub value: String,
    /// A literal is being held back. The panel shows that the key is set; a save
    /// that leaves it untouched keeps it.
    pub secret: bool,
}

impl McpValue {
    /// Splits a stored value into what is safe to show and what is not.
    ///
    /// The test is whether it names a variable rather than holding one. A value
    /// that is entirely `${…}` is a name, and showing it is the whole reason the
    /// syntax exists — someone has to be able to see *which* variable a server
    /// wants. Anything else is treated as a credential, including a value that
    /// merely contains a reference, because `Bearer ${TOKEN}` and
    /// `Bearer sk-live-…` are the same shape from here.
    fn from_stored(key: &str, value: &str) -> Self {
        let is_reference =
            value.starts_with("${") && value.ends_with('}') && !value[2..].contains("${");
        Self {
            key: key.to_string(),
            value: if is_reference {
                value.to_string()
            } else {
                String::new()
            },
            secret: !is_reference,
        }
    }
}

/// One configured server, as the panel renders it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServerView {
    pub name: String,
    /// The layer that defines it, and so the file a save writes to.
    pub scope: Scope,
    pub transport: McpTransport,
    /// Stdio only.
    pub command: String,
    /// Stdio only.
    pub args: Vec<String>,
    /// Stdio only.
    pub env: Vec<McpValue>,
    /// HTTP only.
    pub url: String,
    /// HTTP only.
    pub headers: Vec<McpValue>,
    pub disabled: bool,
    /// Where `command` resolves on this application's PATH.
    ///
    /// The question behind almost every stdio failure, answered before the
    /// server is started rather than after it fails. `None` on an entry that has
    /// no command, and on one whose command is not installed — which is the case
    /// worth showing.
    #[ts(optional)]
    pub program: Option<String>,
    /// How the last connect went. `None` before the first reload has run.
    #[ts(optional)]
    pub status: Option<ServerStatus>,
}

impl McpServerView {
    pub fn new(
        name: String,
        server: ServerConfig,
        defined_in: &LayerOf,
        status: Option<ServerStatus>,
    ) -> Self {
        // Falls back to the global layer for a server that exists only as a
        // toggle. Editing one writes a real definition, and the global file is
        // where a definition with nothing to override belongs.
        let scope = defined_in.get(&name).copied().unwrap_or(Scope::Global);
        let values = |map: &BTreeMap<String, String>| {
            map.iter()
                .map(|(k, v)| McpValue::from_stored(k, v))
                .collect()
        };

        match server {
            ServerConfig::Stdio {
                command,
                args,
                env,
                disabled,
            } => Self {
                program: resolve_program(&command),
                name,
                scope,
                transport: McpTransport::Stdio,
                command,
                args,
                env: values(&env),
                url: String::new(),
                headers: Vec::new(),
                disabled,
                status,
            },
            ServerConfig::Http {
                url,
                headers,
                disabled,
            } => Self {
                name,
                scope,
                transport: McpTransport::Http,
                command: String::new(),
                args: Vec::new(),
                env: Vec::new(),
                url,
                headers: values(&headers),
                disabled,
                program: None,
                status,
            },
            // Merging resolves a toggle against the server it names, so one that
            // survives to here named nothing. Rendered as an empty stdio entry so
            // the panel can show it, say it is not defined, and let it be fixed
            // in place rather than only in the file.
            ServerConfig::Toggle(toggle) => Self {
                name,
                scope,
                transport: McpTransport::Stdio,
                command: String::new(),
                args: Vec::new(),
                env: Vec::new(),
                url: String::new(),
                headers: Vec::new(),
                disabled: toggle.disabled,
                program: None,
                status,
            },
        }
    }
}

/// The entry a save or a test is working from, when that is not simply the one
/// being written.
///
/// A rename and a move between layers are the same operation from here: the
/// entry that has to be read for its held-back secrets, and then removed, is not
/// the one at the draft's own name and scope. Carrying both together rather than
/// as two optional fields is what stops one being passed without the other,
/// which would look up the right name in the wrong file.
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct McpServerRef {
    pub scope: Scope,
    pub name: String,
}

/// What the panel sends when a server is saved.
///
/// Separate from [`McpServerView`] because it carries less: no status, no
/// resolved program, and values that may be the "unchanged" marker. A save is
/// not the inverse of a read here, and one type pretending to be both would make
/// the secret rule invisible.
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct McpServerDraft {
    pub name: String,
    pub scope: Scope,
    pub transport: McpTransport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<McpValue>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<McpValue>,
    #[serde(default)]
    pub disabled: bool,
}

impl McpServerDraft {
    /// Where this draft would live if it were not replacing anything.
    ///
    /// The fallback when the panel sends no `previous`, which is what adding a
    /// server does.
    pub fn origin(&self) -> McpServerRef {
        McpServerRef {
            scope: self.scope,
            name: self.name.trim().to_string(),
        }
    }

    /// The entry to write, with held-back secrets taken from `existing`.
    pub fn to_config(&self, existing: Option<&ServerConfig>) -> ServerConfig {
        match self.transport {
            McpTransport::Stdio => ServerConfig::Stdio {
                command: self.command.trim().to_string(),
                args: self
                    .args
                    .iter()
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect(),
                env: merge_secrets(&self.env, stored_env(existing)),
                disabled: self.disabled,
            },
            McpTransport::Http => ServerConfig::Http {
                url: self.url.trim().to_string(),
                headers: merge_secrets(&self.headers, stored_headers(existing)),
                disabled: self.disabled,
            },
        }
    }
}

fn stored_env(existing: Option<&ServerConfig>) -> Option<&BTreeMap<String, String>> {
    match existing {
        Some(ServerConfig::Stdio { env, .. }) => Some(env),
        _ => None,
    }
}

fn stored_headers(existing: Option<&ServerConfig>) -> Option<&BTreeMap<String, String>> {
    match existing {
        Some(ServerConfig::Http { headers, .. }) => Some(headers),
        _ => None,
    }
}

/// Applies the incoming values, keeping the stored secret wherever one was held
/// back and nothing was typed in its place.
///
/// The failure this exists to prevent is a save that silently blanks a token
/// because the form never had it to send back. A key that is renamed loses its
/// held-back value, which is correct: it is a different key, and there is no
/// stored secret for it.
fn merge_secrets(
    incoming: &[McpValue],
    stored: Option<&BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for entry in incoming {
        let key = entry.key.trim();
        if key.is_empty() {
            continue;
        }
        let value = if entry.secret && entry.value.is_empty() {
            match stored.and_then(|s| s.get(key)) {
                Some(kept) => kept.clone(),
                // Marked secret with nothing behind it: a new key whose value was
                // left blank. Dropped rather than written empty, because an empty
                // credential produces an authentication failure that reads like a
                // wrong one.
                None => continue,
            }
        } else {
            entry.value.clone()
        };
        out.insert(key.to_string(), value);
    }
    out
}

/// Where `command` is on this application's PATH.
///
/// Skipped for a command that is already a path: the answer is either itself or
/// nothing, and `which` says so.
fn resolve_program(command: &str) -> Option<String> {
    if command.trim().is_empty() {
        return None;
    }
    taurus_tools::login_path::which(command.trim()).map(|p: PathBuf| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(env: BTreeMap<String, String>) -> ServerConfig {
        ServerConfig::Stdio {
            command: "npx".into(),
            args: vec![],
            env,
            disabled: false,
        }
    }

    #[test]
    fn a_variable_reference_is_shown_and_a_literal_is_not() {
        // Which variable a server wants is the thing you need to see; what the
        // variable contains is the thing that must not travel.
        let reference = McpValue::from_stored("TOKEN", "${GITHUB_TOKEN}");
        assert_eq!(reference.value, "${GITHUB_TOKEN}");
        assert!(!reference.secret);

        let literal = McpValue::from_stored("TOKEN", "ghp_realtokenvalue");
        assert_eq!(literal.value, "");
        assert!(literal.secret);

        // A value that merely contains a reference is a credential too:
        // `Bearer ${TOKEN}` and `Bearer sk-live-…` are the same shape from here.
        let mixed = McpValue::from_stored("Authorization", "Bearer ${TOKEN}");
        assert_eq!(mixed.value, "");
        assert!(mixed.secret);
    }

    #[test]
    fn saving_an_untouched_secret_keeps_it_rather_than_blanking_it() {
        // The bug this whole arrangement is built to avoid: the form never had
        // the token, so a naive save writes back the empty string it was shown
        // and the server starts failing to authenticate.
        let existing = stdio(BTreeMap::from([(
            "TOKEN".to_string(),
            "ghp_realtokenvalue".to_string(),
        )]));
        let draft = McpServerDraft {
            name: "github".into(),
            scope: Scope::Global,
            transport: McpTransport::Stdio,
            command: "npx".into(),
            args: vec![],
            env: vec![McpValue {
                key: "TOKEN".into(),
                value: String::new(),
                secret: true,
            }],
            url: String::new(),
            headers: vec![],
            disabled: false,
        };

        let ServerConfig::Stdio { env, .. } = draft.to_config(Some(&existing)) else {
            panic!("a stdio draft must write a stdio server");
        };
        assert_eq!(env["TOKEN"], "ghp_realtokenvalue");
    }

    #[test]
    fn typing_a_new_value_over_a_held_back_one_replaces_it() {
        let existing = stdio(BTreeMap::from([(
            "TOKEN".to_string(),
            "old-secret".to_string(),
        )]));
        let draft = McpServerDraft {
            name: "github".into(),
            scope: Scope::Global,
            transport: McpTransport::Stdio,
            command: "npx".into(),
            args: vec![],
            env: vec![McpValue {
                key: "TOKEN".into(),
                value: "${GITHUB_TOKEN}".into(),
                secret: false,
            }],
            url: String::new(),
            headers: vec![],
            disabled: false,
        };

        let ServerConfig::Stdio { env, .. } = draft.to_config(Some(&existing)) else {
            panic!("a stdio draft must write a stdio server");
        };
        assert_eq!(env["TOKEN"], "${GITHUB_TOKEN}");
    }

    #[test]
    fn a_draft_that_replaces_nothing_points_at_its_own_name_and_layer() {
        // What a save falls back to when the panel sends no `previous`, which is
        // what adding a server does. Trimmed, because the name it is looked up
        // by has to be the name it is written under.
        let draft = McpServerDraft {
            name: "  github  ".into(),
            scope: Scope::Workspace,
            transport: McpTransport::Stdio,
            command: "npx".into(),
            args: vec![],
            env: vec![],
            url: String::new(),
            headers: vec![],
            disabled: false,
        };
        let origin = draft.origin();
        assert_eq!(origin.name, "github");
        assert_eq!(origin.scope, Scope::Workspace);
    }

    #[test]
    fn a_new_key_left_blank_is_dropped_rather_than_written_empty() {
        // An empty credential produces an authentication failure that reads like
        // a wrong one, which is a worse half hour than a missing key.
        assert!(merge_secrets(
            &[McpValue {
                key: "TOKEN".into(),
                value: String::new(),
                secret: true,
            }],
            None,
        )
        .is_empty());
    }

    #[test]
    fn blank_keys_and_blank_arguments_are_not_written() {
        // Both come from a form with a spare row in it, which is how a form with
        // an "add another" button always ends.
        let draft = McpServerDraft {
            name: "fs".into(),
            scope: Scope::Global,
            transport: McpTransport::Stdio,
            command: "  npx  ".into(),
            args: vec!["-y".into(), "   ".into(), "server-fs".into()],
            env: vec![McpValue {
                key: "   ".into(),
                value: "x".into(),
                secret: false,
            }],
            url: String::new(),
            headers: vec![],
            disabled: false,
        };

        let ServerConfig::Stdio {
            command, args, env, ..
        } = draft.to_config(None)
        else {
            panic!("a stdio draft must write a stdio server");
        };
        assert_eq!(command, "npx");
        assert_eq!(args, vec!["-y", "server-fs"]);
        assert!(env.is_empty());
    }

    #[test]
    fn a_server_carries_the_layer_it_came_from_so_a_save_goes_back_to_it() {
        // Writing a workspace server into the global file would change every
        // other project, and the panel has no way to know that on its own.
        let defined_in = LayerOf::from([("fs".to_string(), Scope::Workspace)]);
        let view = McpServerView::new("fs".into(), stdio(BTreeMap::new()), &defined_in, None);
        assert_eq!(view.scope, Scope::Workspace);
        assert_eq!(view.transport, McpTransport::Stdio);

        // A server nobody claims — one that exists only as a toggle — defaults to
        // the layer where a definition with nothing to override belongs.
        let orphan = McpServerView::new(
            "ghost".into(),
            stdio(BTreeMap::new()),
            &LayerOf::new(),
            None,
        );
        assert_eq!(orphan.scope, Scope::Global);
    }

    #[test]
    fn a_command_that_exists_is_resolved_and_one_that_does_not_is_not() {
        // The line the panel shows under a stdio server. Its absence is the
        // answer to "why did this not start", which is otherwise only findable
        // after a reload.
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let found = McpServerView::new(
            "s".into(),
            ServerConfig::Stdio {
                command: shell.into(),
                args: vec![],
                env: BTreeMap::new(),
                disabled: false,
            },
            &LayerOf::new(),
            None,
        );
        assert!(
            found.program.is_some(),
            "PATH is {}",
            taurus_tools::login_path::current()
        );

        let missing = McpServerView::new(
            "s".into(),
            ServerConfig::Stdio {
                command: "definitely-not-a-real-program-xyz".into(),
                args: vec![],
                env: BTreeMap::new(),
                disabled: false,
            },
            &LayerOf::new(),
            None,
        );
        assert!(missing.program.is_none());
    }
}
