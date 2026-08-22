//! The `draft_mcp_server` tool.
//!
//! Adding an MCP server means adding a program the harness runs at every
//! launch, before any tool call and outside the permission engine. That is a
//! different act from approving a skill or
//! a sub-agent, and it resists the same treatment: the reviewable artifact for
//! `npx -y @scope/package` is a package name, which says nothing about what the
//! package does. A review card would be asking for a decision nobody in the
//! loop has the information to make.
//!
//! So this writes nothing and connects nothing. The model does the part it is
//! good at — knowing which server exists, what it is called, and which
//! arguments it takes — and hands back a block to paste, the file to paste it
//! into, and what has to be filled in first. Installing stays an act the user
//! performs, in their editor, having read it.
//!
//! # Secrets
//!
//! Values are never carried. A server needing a token gets `env` and `headers`
//! as *names*, and the draft comes back with placeholders for the user to
//! replace. A key the model typed would live in the transcript, and every copy
//! of the transcript, for as long as the conversation is kept.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolResult};

use crate::config::{load, ServerConfig};

/// What a draft writes where a secret would go. Deliberately not a plausible
/// value: a user skimming the block has to notice this is theirs to fill in.
const PLACEHOLDER: &str = "<replace-me>";

pub const DRAFT_MCP_TOOL: &str = "draft_mcp_server";

#[derive(Deserialize, JsonSchema)]
pub struct DraftMcpServerInput {
    /// Short name for the server, e.g. `filesystem`. This is what its tools are
    /// grouped under.
    pub name: String,
    /// For a program spoken to over stdio: the command to run, e.g. `npx`.
    /// Give this or `url`, not both.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments for `command`, e.g. `["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]`.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Names of environment variables the server needs, e.g. `["GITHUB_TOKEN"]`.
    /// Names only — a placeholder is written for the value, and you must never
    /// put a real secret here.
    #[serde(default)]
    pub env: Option<Vec<String>>,
    /// For a streamable-HTTP server: the endpoint URL. Give this or `command`,
    /// not both.
    #[serde(default)]
    pub url: Option<String>,
    /// Names of headers the server needs, e.g. `["Authorization"]`. Names only,
    /// for the same reason as `env`.
    #[serde(default)]
    pub headers: Option<Vec<String>>,
    /// One line on what this server is for and where it came from. Shown to the
    /// user above the block.
    #[serde(default)]
    pub notes: String,
}

pub struct DraftMcpServer {
    /// The config home, so the draft can name the file it belongs in rather
    /// than describing where it might be.
    home: PathBuf,
}

impl DraftMcpServer {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }
}

#[async_trait]
impl Tool for DraftMcpServer {
    fn name(&self) -> &str {
        DRAFT_MCP_TOOL
    }

    fn description(&self) -> &str {
        "Draft an MCP server entry for the user to install. Use it when they ask to add one, or \
         ask what would give you a capability you do not have. This writes nothing and connects \
         nothing — it hands back a JSON block, the file it goes in, and what to fill in — because \
         installing a server means running a program on every launch, and that is the user's \
         decision to make with the entry in front of them. Never put a real token, key, or \
         password in `env` or `headers`: give the *names* only, and the draft leaves a placeholder."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<DraftMcpServerInput>()
    }

    /// Read, and genuinely so: nothing here touches `mcp.json`.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Draft MCP server '{}'",
            input.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        )
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: DraftMcpServerInput = parse_input(input)?;

        if input.name.trim().is_empty() {
            return Err(ToolError::InvalidInput("the server needs a name".into()));
        }

        let placeholders = |names: Option<Vec<String>>| -> BTreeMap<String, String> {
            names
                .unwrap_or_default()
                .into_iter()
                .map(|name| (name, PLACEHOLDER.to_string()))
                .collect()
        };

        let server = match (input.command, input.url) {
            (Some(command), None) => ServerConfig::Stdio {
                command,
                args: input.args.unwrap_or_default(),
                env: placeholders(input.env),
                disabled: false,
            },
            (None, Some(url)) => ServerConfig::Http {
                url,
                headers: placeholders(input.headers),
                disabled: false,
            },
            (Some(_), Some(_)) => {
                return Err(ToolError::InvalidInput(
                    "a server is reached either by running a command or over a URL, not both; \
                     give one"
                        .into(),
                ))
            }
            (None, None) => {
                return Err(ToolError::InvalidInput(
                    "give either `command` (a program spoken to over stdio) or `url` (a \
                     streamable-HTTP endpoint)"
                        .into(),
                ))
            }
        };

        // Rendered through the type the loader actually reads, so a block that
        // comes back here is a block that will parse when it is pasted. A draft
        // that looked right and failed on restart would be worse than none.
        let block = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": { &input.name: server }
        }))
        .map_err(|e| ToolError::Failed(format!("could not render the entry: {e}")))?;

        let path = crate::config::config_file(&self.home);
        let existing = load(&self.home).unwrap_or_default();

        let mut report = String::new();
        if !input.notes.trim().is_empty() {
            report.push_str(input.notes.trim());
            report.push_str("\n\n");
        }
        if let Some(prior) = existing.servers.get(&input.name) {
            report.push_str(&format!(
                "Note: '{}' is already configured as `{}`. Pasting this replaces it.\n\n",
                input.name,
                prior.describe()
            ));
        }
        report.push_str(&format!("Add to {}:\n\n{block}\n", path.display()));
        if block.contains(PLACEHOLDER) {
            report.push_str(&format!(
                "\nEvery `{PLACEHOLDER}` is a value only the user has — fill those in before \
                 saving. Tell them what each one is and where to get it; do not guess at values.\n"
            ));
        }
        report.push_str(
            "\nThis is a draft only: nothing was written and no server was started. The user \
             installs it, and it connects on the next reload.\n",
        );

        Ok(report.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use taurus_tools::{AllowAll, PermissionEngine};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> (ToolContext, TempDir) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let engine = Arc::new(PermissionEngine::new(
            &root,
            root.join(".taurus"),
            Box::new(AllowAll),
        ));
        (
            ToolContext::new(root, engine, CancellationToken::new()),
            dir,
        )
    }

    async fn draft(home: &TempDir, input: serde_json::Value) -> ToolResult {
        let tool = DraftMcpServer::new(home.path().to_path_buf());
        let (ctx, _workspace) = ctx();
        tool.execute(input, &ctx).await
    }

    #[tokio::test]
    async fn a_stdio_draft_comes_back_as_something_that_parses() {
        // The whole value of drafting through the real type: a block that
        // reaches the user has to be one that loads. A draft that looked right
        // and failed on restart would be worse than no draft.
        let home = TempDir::new().unwrap();
        let report = draft(
            &home,
            serde_json::json!({
                "name": "filesystem",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
            }),
        )
        .await
        .unwrap();

        let report = report.to_text();
        assert!(report.contains("npx"), "{report}");
        let block = report
            .split_once("\n\n")
            .map(|(_, rest)| rest)
            .unwrap_or(&report);
        let start = block.find('{').unwrap();
        let end = block.rfind('}').unwrap();
        std::fs::write(crate::config::config_file(home.path()), &block[start..=end]).unwrap();
        let loaded = load(home.path()).unwrap();
        assert!(matches!(
            loaded.servers["filesystem"],
            ServerConfig::Stdio { .. }
        ));
    }

    #[tokio::test]
    async fn a_secret_is_never_carried_only_named() {
        // The reason `env` takes names. A key the model typed would live in the
        // transcript, and in every copy of it, for as long as it is kept.
        let home = TempDir::new().unwrap();
        let report = draft(
            &home,
            serde_json::json!({
                "name": "github",
                "command": "npx",
                "env": ["GITHUB_TOKEN"],
            }),
        )
        .await
        .unwrap();

        assert!(report.to_text().contains("GITHUB_TOKEN"), "{report}");
        assert!(report.to_text().contains(PLACEHOLDER), "{report}");
        assert!(
            report.to_text().contains("only the user has"),
            "the user has to be told to fill it in: {report}"
        );
    }

    #[tokio::test]
    async fn the_draft_names_the_file_it_belongs_in() {
        let home = TempDir::new().unwrap();
        let report = draft(
            &home,
            serde_json::json!({ "name": "remote", "url": "https://example.com/mcp" }),
        )
        .await
        .unwrap();
        assert!(report.to_text().contains("mcp.json"), "{report}");
        assert!(report.to_text().contains("nothing was written"), "{report}");
    }

    #[tokio::test]
    async fn replacing_a_configured_server_is_pointed_out() {
        let home = TempDir::new().unwrap();
        std::fs::write(
            crate::config::config_file(home.path()),
            r#"{"mcpServers": {"github": {"command": "old-binary"}}}"#,
        )
        .unwrap();

        let report = draft(
            &home,
            serde_json::json!({ "name": "github", "command": "npx" }),
        )
        .await
        .unwrap();

        assert!(report.to_text().contains("already configured"), "{report}");
        assert!(report.to_text().contains("old-binary"), "{report}");
    }

    #[tokio::test]
    async fn a_server_needs_exactly_one_way_to_reach_it() {
        let home = TempDir::new().unwrap();
        for input in [
            serde_json::json!({ "name": "x" }),
            serde_json::json!({ "name": "x", "command": "npx", "url": "https://e.com" }),
        ] {
            assert!(draft(&home, input).await.is_err());
        }
    }
}
