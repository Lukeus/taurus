//! MCP client: external tool servers, exposed to the agent as ordinary tools.
//!
//! Everything an MCP server offers is wrapped in the same [`Tool`] trait the
//! built-ins implement and registered in the same registry, so the agent loop
//! and the permission gate treat them identically. The model sees one flat
//! namespace.

pub mod config;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{Peer, RoleClient, RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use tokio::sync::RwLock;
use tracing::{info, warn};

use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolResult};

pub use config::{load, McpConfig, ServerConfig};

/// Prefix that keeps MCP tools from colliding with built-ins or each other.
pub const NAMESPACE: &str = "mcp__";

pub fn namespaced(server: &str, tool: &str) -> String {
    format!("{NAMESPACE}{server}__{tool}")
}

/// A live connection to one server.
///
/// Exists only to own the `RunningService`: dropping it shuts the child
/// process down, so the manager must hold one per server for as long as its
/// tools are registered.
struct Connection {
    _service: RunningService<RoleClient, ()>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ServerStatus {
    pub name: String,
    pub description: String,
    pub connected: bool,
    pub tool_count: usize,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct McpManager {
    connections: RwLock<BTreeMap<String, Arc<Connection>>>,
    status: RwLock<BTreeMap<String, ServerStatus>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Connects to every enabled server and returns the tools they expose.
    ///
    /// A server that fails to start is recorded and skipped rather than
    /// failing the whole load: one broken entry in `mcp.json` must not cost
    /// the user their other servers.
    pub async fn connect_all(&self, config: &McpConfig) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

        for (name, server) in &config.servers {
            if server.disabled() {
                continue;
            }
            match self.connect(name, server).await {
                Ok(mut connected) => {
                    info!(server = %name, tools = connected.len(), "mcp server connected");
                    tools.append(&mut connected);
                }
                Err(e) => {
                    warn!(server = %name, error = %e, "mcp server failed to start");
                    self.status.write().await.insert(
                        name.clone(),
                        ServerStatus {
                            name: name.clone(),
                            description: server.describe(),
                            connected: false,
                            tool_count: 0,
                            error: Some(e),
                        },
                    );
                }
            }
        }
        tools
    }

    async fn connect(
        &self,
        name: &str,
        server: &ServerConfig,
    ) -> Result<Vec<Arc<dyn Tool>>, String> {
        let service = match server {
            ServerConfig::Stdio {
                command, args, env, ..
            } => {
                let transport = TokioChildProcess::new(spawn_command(command).configure(|c| {
                    c.args(args);
                    for (key, value) in env {
                        c.env(key, value);
                    }
                }))
                .map_err(|e| format!("could not start `{command}`: {e}"))?;
                ().serve(transport)
                    .await
                    .map_err(|e| format!("handshake failed: {e}"))?
            }
            ServerConfig::Http { url, headers, .. } => {
                let url = expand_env(url).map_err(|e| format!("url: {e}"))?;
                let transport = StreamableHttpClientTransport::<reqwest::Client>::from_config(
                    StreamableHttpClientTransportConfig::with_uri(url)
                        .custom_headers(http_headers(headers)?),
                );
                ().serve(transport)
                    .await
                    .map_err(|e| format!("handshake failed: {e}"))?
            }
            // Layer merging resolves toggles against the server they name, so
            // one reaching this far means it named nothing.
            ServerConfig::Toggle(_) => {
                return Err(format!(
                    "'{name}' only sets `disabled`; it needs a `command` or a `url`"
                ))
            }
        };

        let peer = service.peer().clone();
        let listed = peer
            .list_all_tools()
            .await
            .map_err(|e| format!("could not list tools: {e}"))?;

        let tools: Vec<Arc<dyn Tool>> = listed
            .into_iter()
            .map(|tool| {
                Arc::new(McpTool {
                    name: namespaced(name, &tool.name),
                    remote_name: tool.name.to_string(),
                    server: name.to_string(),
                    description: tool
                        .description
                        .as_ref()
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| format!("Tool `{}` from the {name} server", tool.name)),
                    schema: serde_json::Value::Object((*tool.input_schema).clone()),
                    peer: peer.clone(),
                }) as Arc<dyn Tool>
            })
            .collect();

        self.status.write().await.insert(
            name.to_string(),
            ServerStatus {
                name: name.to_string(),
                description: server.describe(),
                connected: true,
                tool_count: tools.len(),
                error: None,
            },
        );
        self.connections
            .write()
            .await
            .insert(name.to_string(), Arc::new(Connection { _service: service }));

        Ok(tools)
    }

    pub async fn statuses(&self) -> Vec<ServerStatus> {
        self.status.read().await.values().cloned().collect()
    }

    /// Drops every connection, shutting down child processes.
    pub async fn shutdown(&self) {
        self.connections.write().await.clear();
        self.status.write().await.clear();
    }
}

/// Substitutes `${VAR}` from the environment.
///
/// An HTTP server almost always needs a credential in a header, and the
/// workspace layer of `mcp.json` is meant to be hand-written and committed —
/// a literal token there is a token in the repository. Naming the variable
/// instead is the same bargain `providers.json` already makes for API keys.
///
/// A literal value still passes through untouched, so a config pasted from
/// somewhere else keeps working.
fn expand_env(raw: &str) -> Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err("unterminated `${` — write `${VAR}`".into());
        };
        let name = &after[..end];
        // Unset is an error rather than an empty string: sending a blank
        // Authorization header produces a 401 that looks like a bad token
        // rather than a missing one.
        let value =
            std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    Ok(out)
}

fn http_headers(
    raw: &BTreeMap<String, String>,
) -> Result<HashMap<HeaderName, HeaderValue>, String> {
    let mut headers = HashMap::with_capacity(raw.len());
    for (name, value) in raw {
        let header = HeaderName::try_from(name.as_str())
            .map_err(|_| format!("'{name}' is not a valid HTTP header name"))?;
        let expanded = expand_env(value).map_err(|e| format!("header '{name}': {e}"))?;
        let mut value = HeaderValue::try_from(expanded)
            .map_err(|_| format!("header '{name}' has a value HTTP does not allow"))?;
        // These carry credentials. Marking them keeps the value out of the
        // `{:?}` rendering that reqwest and rmcp use when tracing a request.
        value.set_sensitive(true);
        headers.insert(header, value);
    }
    Ok(headers)
}

/// On Windows, `npx` and friends are batch scripts that `CreateProcess` cannot
/// execute directly; they have to go through the command interpreter. This is
/// the single most common reason an MCP config that works on macOS fails on
/// Windows.
#[cfg(windows)]
fn spawn_command(command: &str) -> tokio::process::Command {
    let needs_shell = matches!(
        std::path::Path::new(command)
            .extension()
            .and_then(|e| e.to_str()),
        Some("cmd") | Some("bat")
    ) || !command.contains('.');
    if needs_shell {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        tokio::process::Command::new(command)
    }
}

#[cfg(not(windows))]
fn spawn_command(command: &str) -> tokio::process::Command {
    tokio::process::Command::new(command)
}

/// One remote tool, wearing the local [`Tool`] interface.
struct McpTool {
    name: String,
    remote_name: String,
    server: String,
    description: String,
    schema: serde_json::Value,
    peer: Peer<RoleClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    /// An MCP server is an external program doing arbitrary work, so its tools
    /// sit in the highest permission tier regardless of what they claim to do.
    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "{} via {} {}",
            self.remote_name,
            self.server,
            taurus_tools::tool::compact(input)
        )
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let arguments = match input {
            serde_json::Value::Object(map) => Some(map),
            serde_json::Value::Null => None,
            other => {
                return Err(ToolError::InvalidInput(format!(
                    "expected an object of arguments, got {other}"
                )))
            }
        };

        let mut params = CallToolRequestParams::default();
        params.name = self.remote_name.clone().into();
        params.arguments = arguments;
        let call = self.peer.call_tool(params);

        let result = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(ToolError::Canceled),
            result = call => result,
        };

        let result = result.map_err(|e| {
            ToolError::Failed(format!(
                "{} on server '{}': {e}",
                self.remote_name, self.server
            ))
        })?;

        let text = render(&result);
        // MCP reports tool-level failure in-band; surface it as an error result
        // so the model can react rather than treating it as success.
        if result.is_error.unwrap_or(false) {
            return Err(ToolError::Failed(text));
        }
        Ok(if text.trim().is_empty() {
            "(no output)".into()
        } else {
            text
        })
    }
}

/// Flattens MCP's content blocks into text for the model.
fn render(result: &rmcp::model::CallToolResult) -> String {
    let mut out = Vec::new();
    for item in &result.content {
        match item {
            ContentBlock::Text(text) => out.push(text.text.clone()),
            ContentBlock::Image(image) => out.push(format!("[image: {}]", image.mime_type)),
            ContentBlock::Audio(audio) => out.push(format!("[audio: {}]", audio.mime_type)),
            // Resources and links carry a URI the model can ask about; the
            // payload itself is usually too large to inline.
            ContentBlock::Resource(_) => out.push("[embedded resource]".into()),
            ContentBlock::ResourceLink(link) => out.push(format!("[resource: {}]", link.uri)),
            // The content set grows with the protocol; an unknown block is
            // reported rather than dropped so the model is not left guessing
            // why a tool returned nothing.
            _ => out.push("[unsupported content block]".into()),
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_namespaced_by_server() {
        assert_eq!(
            namespaced("github", "create_issue"),
            "mcp__github__create_issue"
        );
    }

    #[tokio::test]
    async fn an_empty_config_connects_to_nothing() {
        let manager = McpManager::new();
        assert!(manager.connect_all(&McpConfig::default()).await.is_empty());
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_is_recorded_and_skipped() {
        let manager = McpManager::new();
        let mut config = McpConfig::default();
        config.servers.insert(
            "broken".into(),
            ServerConfig::Stdio {
                command: "definitely-not-a-real-program-xyz".into(),
                args: vec![],
                env: Default::default(),
                disabled: false,
            },
        );

        let tools = manager.connect_all(&config).await;
        assert!(tools.is_empty());

        let statuses = manager.statuses().await;
        assert_eq!(statuses.len(), 1);
        assert!(!statuses[0].connected);
        assert!(statuses[0].error.is_some());
    }

    #[test]
    fn a_header_can_name_an_environment_variable_instead_of_holding_the_secret() {
        // Unique per test: these mutate process-wide state, and the suite runs
        // its tests in parallel threads.
        std::env::set_var("TAURUS_TEST_MCP_TOKEN", "s3cret");
        let headers = http_headers(&BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer ${TAURUS_TEST_MCP_TOKEN}".to_string(),
        )]))
        .unwrap();

        let value = &headers[&HeaderName::from_static("authorization")];
        assert_eq!(value.to_str().unwrap(), "Bearer s3cret");
        assert!(
            value.is_sensitive(),
            "a credential header must not be printable by a stray {{:?}}"
        );
    }

    #[test]
    fn a_literal_header_still_passes_through_unchanged() {
        // The format's whole point is that someone else's config pastes in.
        let headers = http_headers(&BTreeMap::from([(
            "X-Api-Key".to_string(),
            "literal-value".to_string(),
        )]))
        .unwrap();
        assert_eq!(
            headers[&HeaderName::from_static("x-api-key")]
                .to_str()
                .unwrap(),
            "literal-value"
        );
    }

    #[test]
    fn an_unset_variable_names_itself_rather_than_sending_an_empty_credential() {
        let err = http_headers(&BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer ${TAURUS_TEST_MCP_DEFINITELY_UNSET}".to_string(),
        )]))
        .unwrap_err();
        assert!(err.contains("TAURUS_TEST_MCP_DEFINITELY_UNSET"), "{err}");
        assert!(err.contains("Authorization"), "{err}");
    }

    #[test]
    fn a_malformed_placeholder_is_reported_not_sent_verbatim() {
        assert!(expand_env("Bearer ${OOPS").is_err());
        assert_eq!(
            expand_env("no placeholders here").unwrap(),
            "no placeholders here"
        );
    }

    #[tokio::test]
    async fn an_unreachable_http_server_is_recorded_and_skipped() {
        let manager = McpManager::new();
        let mut config = McpConfig::default();
        config.servers.insert(
            "remote".into(),
            ServerConfig::Http {
                // Reserved by RFC 2606, so this cannot accidentally resolve.
                url: "http://mcp.invalid/mcp".into(),
                headers: Default::default(),
                disabled: false,
            },
        );

        assert!(manager.connect_all(&config).await.is_empty());
        let statuses = manager.statuses().await;
        assert_eq!(statuses.len(), 1);
        assert!(!statuses[0].connected);
        assert!(
            statuses[0].error.is_some(),
            "the reason has to reach the user, not just the log"
        );
    }

    #[tokio::test]
    async fn a_disabled_server_is_not_started() {
        let manager = McpManager::new();
        let mut config = McpConfig::default();
        config.servers.insert(
            "off".into(),
            ServerConfig::Stdio {
                command: "definitely-not-a-real-program-xyz".into(),
                args: vec![],
                env: Default::default(),
                disabled: true,
            },
        );
        assert!(manager.connect_all(&config).await.is_empty());
        assert!(
            manager.statuses().await.is_empty(),
            "disabled servers are not attempted"
        );
    }
}
