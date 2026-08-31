//! MCP client: external tool servers, exposed to the agent as ordinary tools.
//!
//! Everything an MCP server offers is wrapped in the same [`Tool`] trait the
//! built-ins implement and registered in the same registry, so the agent loop
//! and the permission gate treat them identically. The model sees one flat
//! namespace.

pub mod catalog;
pub mod config;
pub mod draft;
pub mod oauth;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{Peer, RoleClient, RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use tokio::sync::RwLock;
use tracing::{info, warn};

use taurus_tools::{expand_env, Effect, Tool, ToolContext, ToolError, ToolResult};

pub use catalog::{catalog, Catalog};
pub use config::{load, parse, McpConfig, ServerConfig};
pub use draft::{DraftMcpServer, DRAFT_MCP_TOOL};

/// Prefix that keeps MCP tools from colliding with built-ins or each other.
pub const NAMESPACE: &str = "mcp__";

/// How long one server gets to start up, hand shake, and list its tools.
///
/// There has to be a limit, and it has to be here rather than left to the
/// caller: connecting happens inside `Host::reload`, which the Rescan button
/// awaits, so a server that never answers is a window that never comes back.
/// Generous enough for the common worst case, which is `npx` downloading a
/// package it has not cached yet.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn namespaced(server: &str, tool: &str) -> String {
    format!("{NAMESPACE}{server}__{tool}")
}

/// Whether a tool name belongs to an MCP server.
///
/// Used to lift the MCP tools out of the registry and put a fresh set back
/// without rebuilding everything else — see `Host::reload_mcp`.
pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(NAMESPACE)
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
    /// Switched off in config, so never attempted.
    ///
    /// Recorded rather than skipped: a disabled server used to vanish from the
    /// list entirely, which is indistinguishable from one that was never
    /// configured — so the fix for "why is this not here" was to go and read the
    /// file the panel exists to save you from reading.
    pub disabled: bool,
    /// The remote names of what it offers, unprefixed. What the panel lists
    /// under a connected server, and what a test reports back.
    pub tools: Vec<String>,
}

impl ServerStatus {
    fn failed(name: &str, server: &ServerConfig, error: String) -> Self {
        Self {
            name: name.to_string(),
            description: server.describe(),
            connected: false,
            tool_count: 0,
            error: Some(error),
            disabled: server.disabled(),
            tools: Vec::new(),
        }
    }
}

#[derive(Default)]
pub struct McpManager {
    connections: RwLock<BTreeMap<String, Arc<Connection>>>,
    status: RwLock<BTreeMap<String, ServerStatus>>,
    /// Where OAuth credentials are kept, when the layer above supplied one.
    ///
    /// Injected rather than reached for, because the keychain's platform matrix
    /// is written down once in `taurus_host::secrets` and this crate sits below
    /// that one. `None` leaves every HTTP server unauthenticated, which is what
    /// the CLI and the tests want and is the behaviour that existed before.
    vault: Option<Arc<dyn oauth::TokenVault>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, able to sign in to servers that want OAuth.
    pub fn with_vault(vault: Arc<dyn oauth::TokenVault>) -> Self {
        Self {
            vault: Some(vault),
            ..Self::default()
        }
    }

    /// Whether this server has OAuth credentials stored.
    pub fn signed_in(&self, name: &str) -> bool {
        self.vault
            .as_ref()
            .is_some_and(|vault| oauth::signed_in(vault, name))
    }

    /// Begins a sign-in, returning the URL a browser has to be sent to.
    ///
    /// Two halves, because the middle belongs to whoever can open a window.
    /// See [`oauth::SignIn`].
    pub async fn begin_sign_in(&self, name: &str, url: &str) -> Result<oauth::SignIn, String> {
        let vault = self
            .vault
            .clone()
            .ok_or_else(|| "no credential store on this system to keep a sign-in in".to_string())?;
        oauth::SignIn::begin(url, name, None, vault).await
    }

    /// Forgets one server's credentials. Local only — see [`oauth::sign_out`].
    pub fn sign_out(&self, name: &str) -> Result<(), String> {
        let vault = self
            .vault
            .as_ref()
            .ok_or_else(|| "no credential store on this system".to_string())?;
        oauth::sign_out(vault, name)
    }

    /// Connects to every enabled server and returns the tools they expose.
    ///
    /// A server that fails to start is recorded and skipped rather than
    /// failing the whole load: one broken entry in `mcp.json` must not cost
    /// the user their other servers.
    ///
    /// All of them at once. A connection is almost entirely waiting — spawn
    /// `npx`, wait for it to be ready, then a round trip for `tools/list` — so
    /// serially this cost the *sum* of every server's startup, each with its
    /// own timeout, and that sum sat in front of the first thing the window
    /// asks for. Nothing here contends: each connection touches only its own
    /// entry in the status map.
    ///
    /// The order of the returned tools is still the file's, not the order the
    /// servers happened to answer in — `join_all` yields results in the order
    /// the futures were given. A tool list that reshuffles between launches
    /// would be a prompt that changes for no reason anyone could see.
    pub async fn connect_all(&self, config: &McpConfig) -> Vec<Arc<dyn Tool>> {
        let connecting = config.servers.iter().map(|(name, server)| async move {
            if server.disabled() {
                // Listed but not started. A server that vanished from the status
                // list when switched off looked exactly like one that was never
                // configured at all.
                self.status.write().await.insert(
                    name.clone(),
                    ServerStatus {
                        name: name.clone(),
                        description: server.describe(),
                        connected: false,
                        tool_count: 0,
                        error: None,
                        disabled: true,
                        tools: Vec::new(),
                    },
                );
                return Vec::new();
            }
            match self.connect(name, server).await {
                Ok(connected) => {
                    info!(server = %name, tools = connected.len(), "mcp server connected");
                    connected
                }
                Err(e) => {
                    warn!(server = %name, error = %e, "mcp server failed to start");
                    self.status
                        .write()
                        .await
                        .insert(name.clone(), ServerStatus::failed(name, server, e));
                    Vec::new()
                }
            }
        });

        futures::future::join_all(connecting)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    async fn connect(
        &self,
        name: &str,
        server: &ServerConfig,
    ) -> Result<Vec<Arc<dyn Tool>>, String> {
        let (service, listed) = handshake(name, server, self.vault.clone()).await?;
        let peer = service.peer().clone();

        let tools: Vec<Arc<dyn Tool>> = listed
            .iter()
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
                disabled: false,
                tools: listed.iter().map(|t| t.name.to_string()).collect(),
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

/// Starts a server and asks it what it offers, under one deadline.
///
/// The deadline covers the whole exchange rather than each step: a stdio server
/// that starts instantly and then never answers `tools/list` is the same
/// unusable server as one that never starts, and only one of those two was
/// bounded before.
async fn handshake(
    name: &str,
    server: &ServerConfig,
    // Where OAuth credentials live, when there is anywhere. `None` connects
    // every HTTP server unauthenticated, which is what the CLI and the probe
    // example do.
    vault: Option<Arc<dyn oauth::TokenVault>>,
) -> Result<(RunningService<RoleClient, ()>, Vec<rmcp::model::Tool>), String> {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let service = match server {
            ServerConfig::Stdio {
                command, args, env, ..
            } => {
                // Expanded for the same reason HTTP headers are: the workspace
                // layer of `mcp.json` is meant to be committed, and a server that
                // needs a token would otherwise need that token written into the
                // repository. An unset variable names itself here rather than
                // handing the server an empty string it will report as a bad
                // credential.
                let command = expand_env(command).map_err(|e| format!("command: {e}"))?;
                let mut expanded_args = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    expanded_args.push(expand_env(arg).map_err(|e| format!("args[{i}]: {e}"))?);
                }
                let mut expanded_env = Vec::with_capacity(env.len());
                for (key, value) in env {
                    expanded_env.push((
                        key.clone(),
                        expand_env(value).map_err(|e| format!("env {key}: {e}"))?,
                    ));
                }

                let transport = TokioChildProcess::new(spawn_command(&command).configure(|c| {
                    c.args(&expanded_args);
                    for (key, value) in &expanded_env {
                        c.env(key, value);
                    }
                }))
                .map_err(|e| start_failure(&command, &e))?;
                ().serve(transport)
                    .await
                    .map_err(|e| format!("handshake failed: {e}"))?
            }
            ServerConfig::Http { url, headers, .. } => {
                let url = expand_env(url).map_err(|e| format!("url: {e}"))?;
                let config = StreamableHttpClientTransportConfig::with_uri(url.clone())
                    .custom_headers(http_headers(headers)?);

                /*
                 * Signed in, so every request carries a token that is refreshed
                 * when it needs to be — see `oauth::Authorized`. Nothing here
                 * starts a sign-in: a connection that opened a browser window
                 * because a server answered 401 would be the application taking
                 * over the screen in response to something nobody asked for. A
                 * server that needs signing in fails, says so, and waits.
                 */
                let manager = match vault.clone() {
                    Some(vault) => oauth::manager_for(&url, name, vault).await,
                    None => None,
                };

                match manager {
                    Some(manager) => {
                        let client = oauth::Authorized::new(reqwest::Client::default(), manager);
                        let transport = StreamableHttpClientTransport::with_client(client, config);
                        ().serve(transport)
                            .await
                            .map_err(|e| handshake_failure(e, false))?
                    }
                    None => {
                        let transport =
                            StreamableHttpClientTransport::<reqwest::Client>::from_config(config);
                        ().serve(transport)
                            .await
                            .map_err(|e| handshake_failure(e, true))?
                    }
                }
            }
            // Layer merging resolves toggles against the server they name, so
            // one reaching this far means it named nothing.
            ServerConfig::Toggle(_) => {
                return Err(format!(
                    "'{name}' only sets `disabled`; it needs a `command` or a `url`"
                ))
            }
        };

        let listed = service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| format!("could not list tools: {e}"))?;
        Ok((service, listed))
    })
    .await
    .map_err(|_| {
        format!(
            "did not finish starting within {}s",
            CONNECT_TIMEOUT.as_secs()
        )
    })?
}

/// Why the program would not start, said in terms of what to do about it.
///
/// `NotFound` is the single most common MCP failure and the least self-
/// explanatory: the command is spelled correctly and works in a terminal, and
/// the app cannot see it because a process started from the Dock inherits the
/// launcher's PATH rather than the shell's. Saying which PATH was searched turns
/// that from a mystery into a fact.
fn start_failure(command: &str, error: &std::io::Error) -> String {
    if error.kind() != std::io::ErrorKind::NotFound {
        return format!("could not start `{command}`: {error}");
    }
    format!(
        "`{command}` is not on this application's PATH. It searched: {}. If it is installed by \
         nvm, pyenv, or Homebrew, either restart Taurus so it can read your shell's PATH, or give \
         the full path to the program as `command`.",
        taurus_tools::login_path::current()
    )
}

/// Why a handshake failed, said in terms of what to do about it.
///
/// The one case worth translating is a server that wants somebody signed in.
/// Its raw form is a 401 somewhere inside a transport error, which reads as a
/// misconfiguration and is not one — the entry is correct and the account is
/// missing. `offer_sign_in` is false where credentials were already used, so a
/// 401 there means the grant has been revoked or has expired beyond refresh,
/// which needs signing in *again* rather than for the first time.
fn handshake_failure(error: impl std::fmt::Display, offer_sign_in: bool) -> String {
    let text = error.to_string();
    // Matched on the text because the status is several error types down inside
    // a service error, and the cost of a miss is the raw message rather than a
    // wrong one. Sign in is offered on every unauthenticated HTTP server
    // regardless, so nothing is unreachable if this fails to spot one.
    let unauthorized = text.contains("401")
        || text.contains("Auth required")
        || text.to_lowercase().contains("unauthorized");
    if !unauthorized {
        return format!("handshake failed: {text}");
    }
    if offer_sign_in {
        "this server wants an account. Sign in below, or add a token as an          Authorization header if it issues one."
            .to_string()
    } else {
        "the stored sign-in is no longer accepted — it has expired or been          revoked. Sign in again."
            .to_string()
    }
}

/// Connects to one server, reports what it offers, and disconnects.
///
/// Separate from [`McpManager`] on purpose: this is the Test button, and testing
/// an entry must not register its tools, replace a working connection, or leave
/// a child process behind. The service is dropped at the end of this function,
/// which is what stops the one it started.
pub async fn probe(
    name: &str,
    server: &ServerConfig,
    // So that testing a server you are signed in to tests the thing that
    // actually runs, rather than the unauthenticated half of it.
    vault: Option<Arc<dyn oauth::TokenVault>>,
) -> Result<Vec<String>, String> {
    if server.disabled() {
        // Worth testing anyway — this is how you check an entry before switching
        // it on — so this is a note rather than a refusal.
        info!(server = %name, "probing a disabled server");
    }
    let (service, listed) = handshake(name, server, vault).await?;
    let tools = listed.iter().map(|t| t.name.to_string()).collect();
    // Explicit rather than left to the drop glue, so the child is gone before
    // this returns and a run of tests cannot pile them up.
    let _ = service.cancel().await;
    Ok(tools)
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
/// A stdio server is a long-lived child, so the console window it would
/// otherwise get is not a flicker — it sits on screen for the whole session,
/// one per configured server.
#[cfg(windows)]
fn spawn_command(command: &str) -> tokio::process::Command {
    let needs_shell = matches!(
        std::path::Path::new(command)
            .extension()
            .and_then(|e| e.to_str()),
        Some("cmd") | Some("bat")
    ) || !command.contains('.');
    let mut c = if needs_shell {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        tokio::process::Command::new(command)
    };
    taurus_tools::no_console(&mut c);
    c
}

#[cfg(not(windows))]
fn spawn_command(command: &str) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(command);
    taurus_tools::no_console(&mut c);
    c
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

        let blocks = render(&result);
        // MCP reports tool-level failure in-band; surface it as an error result
        // so the model can react rather than treating it as success. An error
        // is flattened to text on the way out: what a failed call has to say is
        // something to read.
        if result.is_error.unwrap_or(false) {
            return Err(ToolError::Failed(blocks.to_text().into_owned()));
        }
        Ok(blocks)
    }
}

/// MCP's content blocks as this harness's own.
///
/// An image crosses as an image. It used to become the literal words
/// `[image: image/png]`, which is what a server that screenshots a page, renders
/// a diagram, or rasterizes a PDF page had its whole answer reduced to — the
/// normalized types could not express a picture inside a tool result, so there
/// was nowhere for it to go.
///
/// The rest still become text, and each says what it was rather than vanishing.
/// A model told `[audio: audio/wav]` knows the tool worked and that it cannot
/// listen to the answer; one told nothing concludes the tool is broken.
fn render(result: &rmcp::model::CallToolResult) -> taurus_provider::ToolOutput {
    use taurus_provider::ToolResultBlock;

    let mut out: Vec<ToolResultBlock> = Vec::new();
    for item in &result.content {
        match item {
            ContentBlock::Text(text) => out.push(ToolResultBlock::text(text.text.clone())),
            ContentBlock::Image(image) => {
                // Checked rather than trusted. A server's declared mime type is
                // as good a guess as a file extension, and an image rejected
                // here costs a sentence while one rejected by the provider
                // costs a round trip and comes back naming a field.
                match taurus_provider::image::check(&image.mime_type, &image.data) {
                    Ok(_) => out.push(ToolResultBlock::image(
                        image.mime_type.clone(),
                        image.data.clone(),
                    )),
                    Err(rejected) => out.push(ToolResultBlock::text(format!(
                        "[an image this harness cannot pass on: {}]",
                        describe(rejected)
                    ))),
                }
            }
            ContentBlock::Audio(audio) => {
                out.push(ToolResultBlock::text(format!(
                    "[audio: {}, which no model here can listen to]",
                    audio.mime_type
                )));
            }
            // Resources and links carry a URI the model can ask about; the
            // payload itself is usually too large to inline.
            ContentBlock::Resource(_) => out.push(ToolResultBlock::text("[embedded resource]")),
            ContentBlock::ResourceLink(link) => {
                out.push(ToolResultBlock::text(format!("[resource: {}]", link.uri)));
            }
            // The content set grows with the protocol; an unknown block is
            // reported rather than dropped so the model is not left guessing
            // why a tool returned nothing.
            _ => out.push(ToolResultBlock::text("[unsupported content block]")),
        }
    }

    // A server that answered with no content at all said something — that the
    // call worked and produced nothing — and the model has to be able to tell
    // that apart from a tool that hung.
    taurus_provider::ToolOutput::blocks(out)
        .unwrap_or_else(|_| taurus_provider::ToolOutput::text("(no output)"))
}

/// Why an image from a server could not be passed on, in words a model can act
/// on: it is the one that has to decide whether to call the tool differently.
fn describe(rejected: taurus_provider::image::Rejected) -> String {
    use taurus_provider::image::Rejected;
    match rejected {
        Rejected::UnknownFormat => "the format is not one of PNG, JPEG, WebP, or GIF".to_string(),
        Rejected::NotBase64 => "the data is not valid base64".to_string(),
        Rejected::Empty => "it is empty".to_string(),
        Rejected::TooLarge { bytes } => format!(
            "it is {:.1} MB, past the {} MB limit",
            bytes as f64 / (1024.0 * 1024.0),
            taurus_provider::image::MAX_IMAGE_BYTES / (1024 * 1024)
        ),
        Rejected::Mismatch { actual } => {
            format!("it is declared as one type but the bytes are {actual}")
        }
    }
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
    async fn a_spawned_server_command_runs_and_pipes_its_output() {
        // `spawn_command` carries two pieces of platform-specific behavior: the
        // `cmd /C` shim for batch-script launchers, and the flag that stops a
        // console window opening. Both are easy to get wrong in a way that only
        // shows up as a server that will not start on somebody else's machine,
        // so the assertion is simply that what comes back still works.
        let program = if cfg!(windows) { "cmd" } else { "echo" };
        let output = spawn_command(program)
            .configure(|c| {
                if cfg!(windows) {
                    c.args(["/C", "echo hello"]);
                } else {
                    c.arg("hello");
                }
            })
            .output()
            .await
            .expect("a configured server command must still be runnable");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
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
    async fn a_disabled_server_is_listed_but_never_started() {
        // Both halves matter. Starting it would defeat the switch; leaving it
        // out of the list made a server someone had turned off indistinguishable
        // from one that was never configured, and the only way to tell was to go
        // and read the file.
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

        let statuses = manager.statuses().await;
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].disabled);
        assert!(!statuses[0].connected);
        assert!(
            statuses[0].error.is_none(),
            "switched off is not a failure: {:?}",
            statuses[0].error
        );
    }

    #[tokio::test]
    // Unix only, because on Windows a bare command goes through the `cmd /C`
    // shim that `spawn_command` applies for batch-script launchers. `cmd` starts
    // perfectly well and *it* reports the missing program, so the spawn does not
    // fail with `NotFound` and there is nothing here to recognise. Windows also
    // does not have the problem this message is for: a GUI process there
    // inherits the full user PATH from the registry.
    #[cfg(unix)]
    async fn a_missing_program_is_reported_against_the_path_that_was_searched() {
        // The failure this whole feature keeps tripping over. `npx` is spelled
        // correctly and works in a terminal; an app started from the Dock cannot
        // see it. "No such file or directory (os error 2)" sends people to check
        // the spelling, which was never wrong.
        let manager = McpManager::new();
        let mut config = McpConfig::default();
        config.servers.insert(
            "fs".into(),
            ServerConfig::Stdio {
                command: "definitely-not-a-real-program-xyz".into(),
                args: vec![],
                env: Default::default(),
                disabled: false,
            },
        );

        manager.connect_all(&config).await;
        let error = manager.statuses().await[0].error.clone().unwrap();
        assert!(error.contains("not on this application's PATH"), "{error}");
        assert!(error.contains("It searched:"), "{error}");
        assert!(error.contains("full path"), "{error}");
    }

    #[tokio::test]
    async fn a_stdio_server_can_name_an_environment_variable_instead_of_holding_the_secret() {
        // The documented bargain for HTTP headers, which stdio `env` did not
        // keep: the value was passed through literally, so a committed
        // `"${GITHUB_TOKEN}"` reached the server as those fifteen characters and
        // came back as an authentication failure.
        std::env::set_var("TAURUS_TEST_MCP_STDIO_TOKEN", "s3cret");
        let server = ServerConfig::Stdio {
            command: "definitely-not-a-real-program-xyz".into(),
            args: vec!["${TAURUS_TEST_MCP_STDIO_TOKEN}".into()],
            env: BTreeMap::from([(
                "TOKEN".to_string(),
                "${TAURUS_TEST_MCP_STDIO_TOKEN}".to_string(),
            )]),
            disabled: false,
        };
        // A set variable expands and gets out of the way: what comes back is a
        // complaint about the program, never about the variable. Asserted as the
        // absence of the variable's name rather than the presence of any
        // particular message, because how a missing program fails differs by
        // platform — see the test above.
        let error = probe("t", &server, None).await.unwrap_err();
        assert!(
            !error.contains("TAURUS_TEST_MCP_STDIO_TOKEN"),
            "a variable that is set must not be reported as a problem: {error}"
        );

        let unset = ServerConfig::Stdio {
            command: "echo".into(),
            args: vec![],
            env: BTreeMap::from([(
                "TOKEN".to_string(),
                "${TAURUS_TEST_MCP_STDIO_DEFINITELY_UNSET}".to_string(),
            )]),
            disabled: false,
        };
        let error = probe("t", &unset, None).await.unwrap_err();
        assert!(
            error.contains("TAURUS_TEST_MCP_STDIO_DEFINITELY_UNSET"),
            "{error}"
        );
        assert!(
            error.contains("TOKEN"),
            "the key has to be named too: {error}"
        );
    }

    #[tokio::test]
    async fn a_probe_reports_without_registering_anything() {
        // What the Test button is allowed to do. A test that replaced the live
        // connection would mean checking an edit could take a working server
        // down.
        let manager = McpManager::new();
        let server = ServerConfig::Http {
            // Reserved by RFC 2606, so this cannot accidentally resolve.
            url: "http://mcp.invalid/mcp".into(),
            headers: Default::default(),
            disabled: false,
        };
        assert!(probe("remote", &server, None).await.is_err());
        assert!(
            manager.statuses().await.is_empty(),
            "a probe must leave the manager untouched"
        );
    }

    #[test]
    fn mcp_tools_are_recognisable_by_name_alone() {
        // What lets `reload_mcp` swap the MCP tools out of a registry without
        // rebuilding the built-ins, the skill tools, and the web tools with them.
        assert!(is_mcp_tool(&namespaced("github", "create_issue")));
        assert!(!is_mcp_tool("read_file"));
        assert!(!is_mcp_tool(DRAFT_MCP_TOOL));
    }

    /// Builds a `CallToolResult` the way a server would.
    fn result(content: Vec<ContentBlock>) -> rmcp::model::CallToolResult {
        let mut result = rmcp::model::CallToolResult::success(content);
        result.is_error = Some(false);
        result
    }

    fn png() -> String {
        use base64::Engine;
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(b"and then some pixels");
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn a_screenshot_from_a_server_crosses_as_a_picture() {
        // The gap this whole change exists to close. A server that screenshots
        // a page used to have its entire answer reduced to the eight characters
        // `[image: image/png]`, because there was nowhere for a picture to go.
        let data = png();
        let out = render(&result(vec![
            ContentBlock::text("here is the page"),
            ContentBlock::image(data.clone(), "image/png"),
        ]));

        assert_eq!(out.as_slice().len(), 2);
        assert_eq!(
            out.images().collect::<Vec<_>>(),
            vec![("image/png", data.as_str())]
        );
    }

    #[test]
    fn an_image_the_harness_cannot_send_says_why_instead_of_vanishing() {
        // A server's declared type is as good a guess as a file extension. The
        // block is replaced rather than dropped: a tool whose only answer
        // disappeared reads as a tool that did not work.
        let out = render(&result(vec![ContentBlock::image(
            "not base64 at all!",
            "image/png",
        )]));

        assert!(!out.has_images());
        let text = out.to_text();
        assert!(text.contains("cannot pass on"), "{text}");
        assert!(text.contains("base64"), "{text}");
    }

    #[test]
    fn what_cannot_be_carried_still_says_what_it_was() {
        // A model told `[audio: audio/wav]` knows the call worked and that it
        // cannot listen to the answer. One told nothing concludes it is broken.
        let out = render(&result(vec![ContentBlock::audio("AAAA", "audio/wav")]));
        let text = out.to_text();
        assert!(text.contains("audio/wav"), "{text}");
    }

    #[test]
    fn a_server_that_answered_with_nothing_says_so() {
        // Distinguishable from a call that hung, which is the point.
        let out = render(&result(Vec::new()));
        assert_eq!(out.as_text(), Some("(no output)"));
    }
}
