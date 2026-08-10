//! MCP server configuration.
//!
//! The schema is deliberately the one Claude Desktop and Claude Code use, so
//! an existing `mcpServers` block pastes in unchanged. Adopting someone else's
//! format is worth more here than designing a nicer one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers")]
    pub servers: BTreeMap<String, ServerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerConfig {
    /// A program spoken to over stdio.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        disabled: bool,
    },
    /// A streamable-HTTP endpoint.
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        disabled: bool,
    },
    /// A bare `{"disabled": true}`: not a server, but a change to one defined
    /// in a lower layer. It exists so a workspace can switch off an inherited
    /// server without copying its command line down, which would then rot.
    Toggle(DisableToggle),
}

/// The payload of [`ServerConfig::Toggle`].
///
/// A separate struct purely so `deny_unknown_fields` can apply — serde allows
/// it on a type but not on an enum variant, and without it this last untagged
/// variant would match any object at all, swallowing entries that merely failed
/// to parse as a real server.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
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
        let config: McpConfig =
            serde_json::from_str(r#"{"mcpServers": {"off": {"disabled": true}}}"#).unwrap();
        assert!(matches!(config.servers["off"], ServerConfig::Toggle { .. }));

        // A real entry must not fall through to the toggle variant just
        // because every one of its own fields happens to be optional.
        let config: McpConfig =
            serde_json::from_str(r#"{"mcpServers": {"real": {"command": "x", "disabled": true}}}"#)
                .unwrap();
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
}
