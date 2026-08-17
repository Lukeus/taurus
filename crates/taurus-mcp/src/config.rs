//! MCP server configuration.
//!
//! The schema is deliberately the one Claude Desktop and Claude Code use, so
//! an existing `mcpServers` block pastes in unchanged. Adopting someone else's
//! format is worth more here than designing a nicer one.
//!
//! # One bad entry is one bad entry
//!
//! Every server is parsed on its own. Deserializing the file into one map meant
//! that a typo in the fourth server discarded the three above it, and said so
//! with serde's message for an untagged enum — "data did not match any variant
//! of untagged enum ServerConfig" — which names neither the server nor the key.
//! The whole file still has to be valid JSON, because past that point there is
//! nothing to salvage. Everything after it degrades per entry: the servers that
//! parse are used, and the ones that do not are reported by name with what is
//! actually wrong with them.
//!
//! # Editing
//!
//! [`upsert_entry`], [`remove_entry`], and [`set_entry_disabled`] rewrite one
//! key of the file and copy the rest through as raw JSON. That is what lets the
//! MCP panel change a server Taurus understands without destroying one it does
//! not — an entry using a key from a newer version of the format, or one that is
//! mid-edit and broken, survives a save to its neighbour.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The key the whole format hangs off, in every client that speaks it.
const SERVERS_KEY: &str = "mcpServers";

#[derive(Debug, Default, Serialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub servers: BTreeMap<String, ServerConfig>,
    /// Entries that would not parse, by name, each with what is wrong.
    ///
    /// Not written back out — these are diagnostics, and the text they describe
    /// is still in the file the user wrote.
    #[serde(skip)]
    pub invalid: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ServerConfig {
    /// A program spoken to over stdio.
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "is_false")]
        disabled: bool,
    },
    /// A streamable-HTTP endpoint.
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "is_false")]
        disabled: bool,
    },
    /// A bare `{"disabled": true}`: not a server, but a change to one defined
    /// in a lower layer. It exists so a workspace can switch off an inherited
    /// server without copying its command line down, which would then rot.
    Toggle(DisableToggle),
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The payload of [`ServerConfig::Toggle`].
///
/// A separate struct purely so `deny_unknown_fields` can apply — serde allows
/// it on a type but not on an enum variant, and without it this last untagged
/// variant would match any object at all, swallowing entries that merely failed
/// to parse as a real server.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DisableToggle {
    pub disabled: bool,
}

impl ServerConfig {
    pub fn disabled(&self) -> bool {
        match self {
            Self::Stdio { disabled, .. } | Self::Http { disabled, .. } => *disabled,
            Self::Toggle(toggle) => toggle.disabled,
        }
    }

    pub fn set_disabled(&mut self, value: bool) {
        match self {
            Self::Stdio { disabled, .. } | Self::Http { disabled, .. } => *disabled = value,
            Self::Toggle(toggle) => toggle.disabled = value,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Stdio { command, args, .. } => format!("{command} {}", args.join(" "))
                .trim_end()
                .to_string(),
            Self::Http { url, .. } => url.clone(),
            Self::Toggle(_) => "(not defined)".to_string(),
        }
    }

    /// What is wrong with this entry as something to save, if anything.
    ///
    /// Checked before a write rather than only at connect time, so the panel can
    /// refuse a blank command while the form is still on screen. It deliberately
    /// does not check that the command *exists* — that is a different question,
    /// asked of the PATH, and a server whose program is not installed yet is
    /// still an entry worth saving.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Stdio { command, .. } if command.trim().is_empty() => {
                Err("a stdio server needs a command to run".into())
            }
            Self::Http { url, .. } if url.trim().is_empty() => {
                Err("an HTTP server needs a URL".into())
            }
            Self::Http { url, .. }
                if !url.starts_with("http://")
                    && !url.starts_with("https://")
                    && !url.contains("${") =>
            {
                Err(format!("'{url}' is not an http:// or https:// URL"))
            }
            Self::Toggle(_) => {
                Err("this entry only sets `disabled`; it needs a `command` or a `url`".into())
            }
            _ => Ok(()),
        }
    }
}

pub fn config_file(home: &Path) -> PathBuf {
    home.join("mcp.json")
}

/// Reads `~/.taurus/mcp.json`. A missing file means no servers, not an error.
pub fn load(home: &Path) -> Result<McpConfig, String> {
    let path = config_file(home);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(McpConfig::default());
    };
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parses one layer's text. Only unusable JSON is an error.
///
/// A server that will not parse becomes an entry in [`McpConfig::invalid`]
/// carrying the reason, and its neighbours are unaffected.
pub fn parse(text: &str) -> Result<McpConfig, String> {
    // An empty file is a file someone created and has not filled in — which is
    // exactly what `ensure_mcp_file` leaves behind before the editor opens.
    if text.trim().is_empty() {
        return Ok(McpConfig::default());
    }

    let root: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Some(object) = root.as_object() else {
        return Err(format!(
            "the file must hold a JSON object with a \"{SERVERS_KEY}\" key, not {}",
            type_name(&root)
        ));
    };
    let Some(raw) = object.get(SERVERS_KEY) else {
        // Not an error: a file with other keys and no servers is a file with no
        // servers. Erroring here would break the empty starter file.
        return Ok(McpConfig::default());
    };
    let Some(entries) = raw.as_object() else {
        return Err(format!(
            "\"{SERVERS_KEY}\" must be an object of servers keyed by name, not {}",
            type_name(raw)
        ));
    };

    let mut config = McpConfig::default();
    for (name, value) in entries {
        match serde_json::from_value::<ServerConfig>(value.clone()) {
            Ok(server) => {
                config.servers.insert(name.clone(), server);
            }
            Err(_) => {
                config.invalid.insert(name.clone(), diagnose(value));
            }
        }
    }
    Ok(config)
}

/// Every key this format knows, for spotting a near miss.
const KNOWN_KEYS: &[&str] = &[
    "command",
    "args",
    "env",
    "url",
    "headers",
    "disabled",
    "type",
    "transport",
];

/// Why one entry did not parse, in terms of the entry rather than of serde.
///
/// Reached only after `from_value` has already failed, so it does not have to
/// prove the entry is broken — only to find the most useful thing to say about
/// it. The checks are ordered by how often each is the actual mistake.
fn diagnose(value: &serde_json::Value) -> String {
    let Some(object) = value.as_object() else {
        return format!(
            "must be an object like {{\"command\": \"npx\", \"args\": [\"-y\", \"…\"]}}, not {}",
            type_name(value)
        );
    };

    let has = |key: &str| object.get(key).filter(|v| !v.is_null());
    let string_problem = |key: &str| match has(key) {
        Some(v) if !v.is_string() => {
            Some(format!("`{key}` must be a string, not {}", type_name(v)))
        }
        _ => None,
    };

    if let Some(problem) = string_problem("command").or_else(|| string_problem("url")) {
        return problem;
    }
    if has("command").is_some() && has("url").is_some() {
        return "has both `command` and `url`; a server is reached one way or the other".into();
    }
    if let Some(args) = has("args") {
        if !args.is_array() {
            return format!(
                "`args` must be a list of strings, not {} — a whole command line goes in the list, \
                 one word per item",
                type_name(args)
            );
        }
        if let Some(bad) = args
            .as_array()
            .and_then(|a| a.iter().find(|v| !v.is_string()))
        {
            return format!(
                "every item in `args` must be a string; found {}. Numbers and flags are written \
                 as strings, like \"8080\"",
                type_name(bad)
            );
        }
    }
    for key in ["env", "headers"] {
        if let Some(map) = has(key) {
            let Some(map) = map.as_object() else {
                return format!(
                    "`{key}` must be an object of names to values, not {}",
                    type_name(map)
                );
            };
            if let Some((name, bad)) = map.iter().find(|(_, v)| !v.is_string()) {
                return format!("`{key}.{name}` must be a string, not {}", type_name(bad));
            }
        }
    }
    if let Some(kind) = has("type").and_then(|v| v.as_str()) {
        if kind.eq_ignore_ascii_case("sse") {
            return "`\"type\": \"sse\"` is not supported; Taurus speaks streamable HTTP, so give \
                    the server's `url` without a `type`"
                .into();
        }
    }
    if let Some(disabled) = has("disabled") {
        if !disabled.is_boolean() {
            return format!(
                "`disabled` must be true or false, not {}",
                type_name(disabled)
            );
        }
    }

    // Nothing individually wrong, so the entry says nothing about how to reach
    // the server. Overwhelmingly a misspelled key, and naming the near miss is
    // the difference between a fix and a stare.
    if has("command").is_none() && has("url").is_none() {
        let strays: Vec<String> = object
            .keys()
            .filter(|k| !KNOWN_KEYS.contains(&k.as_str()))
            .map(|k| match nearest_known(k) {
                Some(known) => format!("`{k}` (did you mean `{known}`?)"),
                None => format!("`{k}`"),
            })
            .collect();
        if !strays.is_empty() {
            return format!(
                "has no `command` or `url`, and does not recognise {}",
                strays.join(", ")
            );
        }
        return "needs either `command` (a program spoken to over stdio) or `url` (a \
                streamable-HTTP endpoint)"
            .into();
    }

    "does not match the mcpServers format; see docs/configuration.md#mcp-servers".into()
}

/// The known key `candidate` was probably meant to be.
///
/// Two edits, which covers the mistakes people actually make — `commnd`, `cmd`,
/// `Command`, `arg` — without pairing unrelated four-letter keys.
fn nearest_known(candidate: &str) -> Option<&'static str> {
    let lower = candidate.to_ascii_lowercase();
    KNOWN_KEYS
        .iter()
        .map(|known| (*known, distance(&lower, known)))
        .filter(|(_, d)| *d <= 2)
        .min_by_key(|(_, d)| *d)
        .map(|(known, _)| known)
}

/// Levenshtein distance, two rows at a time.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0usize; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            row[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

fn type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a true/false value",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Adds or replaces one server, leaving every other byte of meaning alone.
///
/// The file is reparsed as raw JSON rather than through [`McpConfig`], so an
/// entry Taurus cannot read — a key from a newer version of the format, a server
/// half-edited by hand — is copied through untouched instead of being dropped by
/// the save of its neighbour.
pub fn upsert_entry(text: &str, name: &str, server: &ServerConfig) -> Result<String, String> {
    server.validate()?;
    edit(text, |servers| {
        servers.insert(
            name.to_string(),
            serde_json::to_value(server).map_err(|e| e.to_string())?,
        );
        Ok(())
    })
}

/// Removes one server. Removing one that is not there is not an error — the
/// requested state is the state that results.
pub fn remove_entry(text: &str, name: &str) -> Result<String, String> {
    edit(text, |servers| {
        servers.remove(name);
        Ok(())
    })
}

/// Flips one server's `disabled` without rewriting the rest of its entry.
///
/// Deliberately a raw-key edit rather than a round trip through
/// [`ServerConfig`]: a toggle must not be the thing that silently drops a field
/// this version does not model.
pub fn set_entry_disabled(text: &str, name: &str, disabled: bool) -> Result<String, String> {
    edit(text, |servers| {
        let Some(entry) = servers.get_mut(name) else {
            return Err(format!("'{name}' is not in this file"));
        };
        let Some(object) = entry.as_object_mut() else {
            return Err(format!(
                "'{name}' is not an object, so it has no `disabled`"
            ));
        };
        object.insert("disabled".into(), serde_json::Value::Bool(disabled));
        Ok(())
    })
}

/// The shared read-modify-write: parse, hand the servers map to `change`,
/// re-render.
fn edit(
    text: &str,
    change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), String>,
) -> Result<String, String> {
    let mut root: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|e| {
            format!(
                "this file is not valid JSON, so editing it here would lose what is in it ({e}). \
                 Fix it in an editor first."
            )
        })?
    };

    // Named before the borrow, because the message is about the value the
    // borrow is being taken from.
    let root_kind = type_name(&root);
    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("the file must hold a JSON object, not {root_kind}"))?;
    let servers = object
        .entry(SERVERS_KEY)
        .or_insert_with(|| serde_json::json!({}));
    let servers_kind = type_name(servers);
    let servers = servers.as_object_mut().ok_or_else(|| {
        format!("\"{SERVERS_KEY}\" must be an object of servers keyed by name, not {servers_kind}")
    })?;

    change(servers)?;

    // Trailing newline: these files are hand-edited and version-controlled, and
    // a missing one is a diff on every save.
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_the_claude_desktop_format_unchanged() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            config_file(dir.path()),
            r#"{
              "mcpServers": {
                "filesystem": {
                  "command": "npx",
                  "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
                },
                "remote": { "url": "https://example.com/mcp" }
              }
            }"#,
        )
        .unwrap();

        let config = load(dir.path()).unwrap();
        assert_eq!(config.servers.len(), 2);
        assert!(matches!(
            config.servers["filesystem"],
            ServerConfig::Stdio { .. }
        ));
        assert!(matches!(
            config.servers["remote"],
            ServerConfig::Http { .. }
        ));
        assert_eq!(
            config.servers["filesystem"].describe(),
            "npx -y @modelcontextprotocol/server-filesystem /tmp"
        );
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        assert!(load(dir.path()).unwrap().servers.is_empty());
    }

    #[test]
    fn malformed_json_is_reported_with_its_path() {
        let dir = TempDir::new().unwrap();
        std::fs::write(config_file(dir.path()), "{ not json").unwrap();
        assert!(load(dir.path()).unwrap_err().contains("mcp.json"));
    }

    #[test]
    fn a_bare_disabled_entry_parses_as_a_toggle_and_nothing_else_does() {
        let config = parse(r#"{"mcpServers": {"off": {"disabled": true}}}"#).unwrap();
        assert!(matches!(config.servers["off"], ServerConfig::Toggle { .. }));

        // A real entry must not fall through to the toggle variant just
        // because every one of its own fields happens to be optional.
        let config =
            parse(r#"{"mcpServers": {"real": {"command": "x", "disabled": true}}}"#).unwrap();
        assert!(matches!(config.servers["real"], ServerConfig::Stdio { .. }));
    }

    #[test]
    fn a_disabled_server_is_marked() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            config_file(dir.path()),
            r#"{"mcpServers": {"off": {"command": "x", "disabled": true}}}"#,
        )
        .unwrap();
        assert!(load(dir.path()).unwrap().servers["off"].disabled());
    }

    #[test]
    fn one_broken_entry_does_not_take_its_neighbours_with_it() {
        // The whole point of parsing per entry. Before this, a typo in the
        // fourth server silently removed the three that worked, and every one of
        // their tools with it.
        let config = parse(
            r#"{"mcpServers": {
                 "good": {"command": "npx"},
                 "typo": {"commnd": "npx"},
                 "also-good": {"url": "https://example.com/mcp"}
               }}"#,
        )
        .unwrap();

        assert_eq!(config.servers.len(), 2);
        assert!(config.servers.contains_key("good"));
        assert!(config.servers.contains_key("also-good"));
        assert_eq!(config.invalid.len(), 1);
        assert!(
            config.invalid["typo"].contains("commnd"),
            "{:?}",
            config.invalid
        );
    }

    #[test]
    fn a_misspelled_key_is_named_along_with_what_it_nearly_is() {
        // The message people actually get to act on. Serde's was "data did not
        // match any variant of untagged enum ServerConfig", which names neither
        // the server nor the key.
        for (raw, expected) in [
            (r#"{"commnd": "npx"}"#, "command"),
            (r#"{"cmd": "npx"}"#, "command"),
            (r#"{"urls": "https://e.com"}"#, "url"),
        ] {
            let message = diagnose(&serde_json::from_str(raw).unwrap());
            assert!(message.contains(expected), "{raw} -> {message}");
        }
    }

    #[test]
    fn the_common_type_mistakes_each_say_which_key_and_what_it_wanted() {
        for (raw, expected) in [
            (r#"{"command": ["npx"]}"#, "`command` must be a string"),
            (
                r#"{"command": "npx", "args": "-y pkg"}"#,
                "`args` must be a list",
            ),
            (
                r#"{"command": "npx", "args": ["-p", 8080]}"#,
                "must be a string",
            ),
            (
                r#"{"url": "https://e.com", "headers": {"A": 1}}"#,
                "`headers.A`",
            ),
            (
                r#"{"command": "npx", "disabled": "yes"}"#,
                "`disabled` must be true or false",
            ),
            (
                r#"{"command": "npx", "url": "https://e.com"}"#,
                "both `command` and `url`",
            ),
            (r#"["npx"]"#, "must be an object"),
        ] {
            let message = diagnose(&serde_json::from_str(raw).unwrap());
            assert!(message.contains(expected), "{raw} -> {message}");
        }
    }

    #[test]
    fn an_sse_entry_is_told_what_taurus_speaks_instead() {
        // Claude Code's config carries `"type"`, and `sse` is the one value of it
        // that changes the protocol. Parsing it as streamable HTTP and failing at
        // handshake time reads as an unreachable server rather than an
        // unsupported transport.
        let message = diagnose(&serde_json::json!({"type": "sse", "urll": "https://e.com/sse"}));
        assert!(message.contains("streamable HTTP"), "{message}");
    }

    #[test]
    fn a_type_key_from_claude_code_is_otherwise_ignored_rather_than_rejected() {
        // The format's whole value is that someone else's block pastes in.
        let config = parse(
            r#"{"mcpServers": {
                 "fs": {"type": "stdio", "command": "npx", "args": ["-y", "server-fs"]},
                 "remote": {"type": "http", "url": "https://example.com/mcp"}
               }}"#,
        )
        .unwrap();
        assert_eq!(config.servers.len(), 2, "{:?}", config.invalid);
    }

    #[test]
    fn a_file_with_no_servers_key_is_empty_rather_than_broken() {
        // What `ensure_mcp_file` writes, and what someone leaves behind after
        // deleting their last server.
        assert!(parse("{}").unwrap().servers.is_empty());
        assert!(parse("").unwrap().servers.is_empty());
        assert!(parse(r#"{"mcpServers": {}}"#).unwrap().servers.is_empty());
    }

    #[test]
    fn saving_one_server_leaves_every_other_byte_of_meaning_alone() {
        // The property that makes a panel safe to use on a hand-written file:
        // `mystery` uses a key this version does not model, and must survive a
        // save to the server next to it.
        let before = r#"{
          "mcpServers": {
            "mystery": {"command": "x", "somethingNew": {"deep": true}},
            "old": {"command": "old"}
          },
          "someOtherTopLevelKey": 1
        }"#;

        let after = upsert_entry(
            before,
            "old",
            &ServerConfig::Stdio {
                command: "new".into(),
                args: vec!["--flag".into()],
                env: BTreeMap::new(),
                disabled: false,
            },
        )
        .unwrap();

        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(root["someOtherTopLevelKey"], 1);
        assert_eq!(root["mcpServers"]["mystery"]["somethingNew"]["deep"], true);
        assert_eq!(root["mcpServers"]["old"]["command"], "new");
        assert_eq!(root["mcpServers"]["old"]["args"][0], "--flag");
    }

    #[test]
    fn an_empty_optional_is_not_written_out() {
        // These files are read by people. A saved server carrying `"args": [],
        // "env": {}, "disabled": false` is three lines of nothing on every entry.
        let after = upsert_entry(
            "",
            "fs",
            &ServerConfig::Stdio {
                command: "npx".into(),
                args: vec![],
                env: BTreeMap::new(),
                disabled: false,
            },
        )
        .unwrap();
        assert!(!after.contains("args"), "{after}");
        assert!(!after.contains("disabled"), "{after}");
        assert!(after.ends_with("}\n"), "{after:?}");
        assert!(parse(&after).unwrap().servers.contains_key("fs"));
    }

    #[test]
    fn a_toggle_survives_a_save_to_another_server() {
        let after = upsert_entry(
            r#"{"mcpServers": {"inherited": {"disabled": true}}}"#,
            "new",
            &ServerConfig::Http {
                url: "https://example.com/mcp".into(),
                headers: BTreeMap::new(),
                disabled: false,
            },
        )
        .unwrap();
        let config = parse(&after).unwrap();
        assert!(matches!(
            config.servers["inherited"],
            ServerConfig::Toggle { .. }
        ));
        assert!(matches!(config.servers["new"], ServerConfig::Http { .. }));
    }

    #[test]
    fn disabling_keeps_the_keys_this_version_does_not_model() {
        let after = set_entry_disabled(
            r#"{"mcpServers": {"fs": {"command": "npx", "somethingNew": 1}}}"#,
            "fs",
            true,
        )
        .unwrap();
        let root: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(root["mcpServers"]["fs"]["disabled"], true);
        assert_eq!(root["mcpServers"]["fs"]["somethingNew"], 1);
    }

    #[test]
    fn removing_one_server_leaves_the_others() {
        let after = remove_entry(
            r#"{"mcpServers": {"a": {"command": "a"}, "b": {"command": "b"}}}"#,
            "a",
        )
        .unwrap();
        let config = parse(&after).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert!(config.servers.contains_key("b"));
        // Removing what is not there is not a failure: the requested state is
        // the state that results.
        assert!(remove_entry(&after, "a").is_ok());
    }

    #[test]
    fn editing_a_file_that_is_not_json_refuses_rather_than_replacing_it() {
        // The one case where a save must not go through. Overwriting would throw
        // away whatever the user was mid-way through typing, which is the only
        // copy of it.
        let error = upsert_entry(
            "{ half-typed",
            "fs",
            &ServerConfig::Stdio {
                command: "npx".into(),
                args: vec![],
                env: BTreeMap::new(),
                disabled: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("not valid JSON"), "{error}");
        assert!(error.contains("editor"), "{error}");
    }

    #[test]
    fn an_entry_with_nothing_to_reach_is_refused_before_it_is_written() {
        assert!(ServerConfig::Stdio {
            command: "  ".into(),
            args: vec![],
            env: BTreeMap::new(),
            disabled: false,
        }
        .validate()
        .is_err());

        assert!(ServerConfig::Http {
            url: "example.com/mcp".into(),
            headers: BTreeMap::new(),
            disabled: false,
        }
        .validate()
        .unwrap_err()
        .contains("http://"));

        // A URL that is entirely an environment variable cannot be checked for a
        // scheme, and refusing it would block the documented way to keep an
        // endpoint out of a version-controlled file.
        assert!(ServerConfig::Http {
            url: "${EXAMPLE_MCP_URL}".into(),
            headers: BTreeMap::new(),
            disabled: false,
        }
        .validate()
        .is_ok());
    }
}
