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
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CancelledNotification, CancelledNotificationParam,
    ClientRequest, ContentBlock, ServerResult,
};
use rmcp::service::{
    Peer, PeerRequestOptions, RoleClient, RunningService, ServiceError, ServiceExt,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use tokio::sync::RwLock;
use tracing::{info, warn};

use taurus_tools::{expand_env, Effect, SecretVault, Tool, ToolContext, ToolError, ToolResult};

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

/// How long a tool call may go without a word before it is given up on.
///
/// An *idle* deadline rather than a total one, and the difference is the whole
/// of the design. `rmcp` restarts the clock whenever the server sends a
/// progress notification, so a server doing an hour of honest work and saying
/// so is never cut off, while one that has wedged is. There is deliberately no
/// ceiling above it: "no tool may take more than N minutes" is a guess about
/// somebody else's build, and **Stop** is always there for the case where the
/// guess would have been right.
///
/// Longer than [`CONNECT_TIMEOUT`] because the two measure different things. A
/// handshake is a round trip and should be quick; a call is the work.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// What one MCP result may take of the model's context.
///
/// The same three numbers the shell tool uses, for the same reason and against
/// the same window. A server is a program nobody here reviewed, returning as
/// much text as it likes into the context every later turn has to carry; that
/// it was written by somebody else is an argument for bounding it, not an
/// exemption from being bounded.
const OUTPUT_SHARE: f32 = 0.08;
const MIN_OUTPUT_BYTES: usize = 4 * 1024;
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

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
    /// Shared with the tools themselves, not merely owned.
    ///
    /// A tool call is the only thing that finds out a server has died — the
    /// manager is not in the loop once the tools are registered — so the tools
    /// need a way to say so. See [`McpTool::note_it_is_gone`].
    status: Arc<RwLock<BTreeMap<String, ServerStatus>>>,
    /// Where OAuth credentials are kept, when the layer above supplied one.
    ///
    /// Injected rather than reached for, because the keychain's platform matrix
    /// is written down once in `taurus_host::secrets` and this crate sits below
    /// that one. `None` leaves every HTTP server unauthenticated, which is what
    /// the CLI and the tests want and is the behaviour that existed before.
    vault: Option<Arc<dyn SecretVault>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, able to sign in to servers that want OAuth.
    pub fn with_vault(vault: Arc<dyn SecretVault>) -> Self {
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
                    status: self.status.clone(),
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
    vault: Option<Arc<dyn SecretVault>>,
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
    vault: Option<Arc<dyn SecretVault>>,
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
    /// The manager's status map, so a call that discovers the server is gone
    /// can correct what the panel is claiming. See [`Self::note_it_is_gone`].
    status: Arc<RwLock<BTreeMap<String, ServerStatus>>>,
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

        // Sent the long way round rather than through `peer.call_tool`, which
        // takes the default options and so has no deadline at all. Everything
        // below is what the default leaves out.
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let options = PeerRequestOptions::with_timeout(CALL_TIMEOUT).reset_timeout_on_progress();
        let handle = match self.peer.send_request_with_option(request, options).await {
            Ok(handle) => handle,
            Err(e) => return Err(self.explain(e).await),
        };

        // Kept before the handle is consumed by the await, because cancelling
        // needs it and `await_response` takes the handle by value.
        let id = handle.id.clone();
        let waiting = handle.await_response();
        tokio::pin!(waiting);
        let answered = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => None,
            answer = &mut waiting => Some(answer),
        };

        let Some(answer) = answered else {
            // Told to stop rather than merely dropped. Dropping the future ends
            // this side's interest in the answer and nothing else: the server
            // goes on computing one nobody will read, which for a child process
            // is a core spinning behind a window that looks idle. The timeout
            // path above sends the same notification, so this is the one case
            // that would otherwise leak.
            let stop = CancelledNotification::new(CancelledNotificationParam::new(
                Some(id),
                Some("the turn was stopped".into()),
            ));
            let _ = self.peer.send_notification(stop.into()).await;
            return Err(ToolError::Canceled);
        };

        let result = match answer {
            Ok(ServerResult::CallToolResult(result)) => result,
            Ok(_) => {
                return Err(ToolError::Failed(format!(
                    "the '{}' server answered {} with something that was not a tool result",
                    self.server, self.remote_name
                )))
            }
            Err(e) => return Err(self.explain(e).await),
        };

        let blocks = fit(&self.name, render(&result), ctx);
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

impl McpTool {
    /// Turns a failed call into something worth reading, and — where the
    /// failure means the server is gone — stops the panel claiming otherwise.
    async fn explain(&self, error: ServiceError) -> ToolError {
        match &error {
            // The transport is the child process's pipe, or the HTTP
            // connection. Either being gone is not a call that failed, it is a
            // server that is no longer there, and every later call will fail
            // the same way until something restarts it.
            ServiceError::TransportClosed | ServiceError::TransportSend(_) => {
                self.note_it_is_gone(&error).await;
                ToolError::Failed(format!(
                    "the '{}' server is no longer running, so {} could not be called. \
                     Press Reconnect in the MCP panel to start it again.",
                    self.server, self.remote_name
                ))
            }
            // Deliberately *not* marked gone. A server that ignored one request
            // for two minutes may well answer the next; saying it is dead on
            // that evidence would be a panel that lies in the other direction,
            // and the recovery — press Reconnect — is one the user can reach
            // for anyway.
            ServiceError::Timeout { timeout } => ToolError::Failed(format!(
                "{} on the '{}' server said nothing for {} seconds and was given up on. \
                 A server that reports progress is not cut off while it works, so this \
                 one has either hung or does not report any.",
                self.remote_name,
                self.server,
                timeout.as_secs()
            )),
            other => ToolError::Failed(format!(
                "{} on server '{}': {other}",
                self.remote_name, self.server
            )),
        }
    }

    /// Corrects the status this server is listed under.
    ///
    /// The status map is written once, at connect, and nothing else revises it
    /// — so a server that died half an hour ago went on being listed as
    /// connected, with a tool count, while every call against it failed. The
    /// panel was the one place a person would go to find out why, and it was
    /// the one place saying nothing was wrong.
    ///
    /// This does not remove the tools from the registry: they were handed out
    /// as `Arc<dyn Tool>` and nothing here can reach back for them. So the
    /// model can still call a tool whose server is gone, and gets the sentence
    /// above for its trouble. Making the two agree means a reconnect, which is
    /// a button rather than something to do behind somebody's back.
    async fn note_it_is_gone(&self, why: &ServiceError) {
        if note_gone(&mut *self.status.write().await, &self.server, why) {
            warn!(server = %self.server, error = %why, "mcp server stopped answering");
        }
    }
}

/// Writes the death into the status map, and says whether it was news.
///
/// Only over a server currently listed as connected. A server that never
/// started already carries the reason it did not, and that reason — the program
/// is not on the PATH, the command is misspelled — is far more useful than
/// "the transport closed", which is only what happens next.
fn note_gone(
    status: &mut BTreeMap<String, ServerStatus>,
    server: &str,
    why: &impl std::fmt::Display,
) -> bool {
    let Some(entry) = status.get_mut(server) else {
        return false;
    };
    if !entry.connected {
        return false;
    }
    entry.connected = false;
    entry.tool_count = 0;
    entry.tools.clear();
    entry.error = Some(format!(
        "stopped answering part way through the session ({why}). Press Reconnect."
    ));
    true
}

/// Bounds what one result may take of the window.
///
/// Text only. An image is already bounded by the provider's own size check on
/// the way in, and cutting one in half produces a broken image rather than a
/// shorter one.
///
/// The full text is written out first where there is anywhere to write it, so
/// the middle of a long answer is a `read_file` away rather than gone — the same
/// bargain the shell tool strikes, and the reason a cut here is worth making at
/// all. `label` names the spill file, and is the namespaced tool name.
///
/// Free rather than a method so a test can reach it: the tool it belongs to
/// carries a live connection to a server, which is not a thing to conjure to
/// check that a long string comes back shorter.
fn fit(
    label: &str,
    blocks: taurus_provider::ToolOutput,
    ctx: &ToolContext,
) -> taurus_provider::ToolOutput {
    use taurus_provider::ToolResultBlock;

    let cap = ctx
        .budget
        .bytes(OUTPUT_SHARE, MIN_OUTPUT_BYTES, MAX_OUTPUT_BYTES);
    let sizes: Vec<usize> = blocks
        .as_slice()
        .iter()
        .filter_map(|block| match block {
            ToolResultBlock::Text { text } => Some(text.len()),
            _ => None,
        })
        .collect();
    if sizes.iter().sum::<usize>() <= cap {
        return blocks;
    }

    let whole = blocks.to_text();
    let spilled = taurus_tools::overflow::spill(&whole, label, ctx);
    let gap = |omitted: usize| match &spilled {
        // `read_file` windows around an offset rather than reading a
        // prefix, so the middle of a large answer is one call away.
        Some(path) => format!(
            "{omitted} bytes omitted; the whole answer was written to {} — read_file it",
            path.display()
        ),
        None => format!("{omitted} bytes omitted"),
    };

    let mut allowances = shares(&sizes, cap).into_iter();
    let kept: Vec<ToolResultBlock> = blocks
        .as_slice()
        .iter()
        .map(|block| match block {
            ToolResultBlock::Text { text } => {
                let allowance = allowances.next().unwrap_or(0);
                ToolResultBlock::text(taurus_tools::overflow::cut(text, allowance, gap))
            }
            other => other.clone(),
        })
        .collect();

    // `blocks` only fails on an empty vector, and this rewrites one block for
    // one block.
    taurus_provider::ToolOutput::blocks(kept).unwrap_or(blocks)
}

/// Divides `cap` between blocks so that only the big ones are cut.
///
/// An even split would take the knife to a two-line block sitting beside a
/// megabyte one, which shortens nothing worth shortening and puts a "bytes
/// omitted" note in the middle of a sentence. So the allowance is filled the
/// other way about: anything already smaller than an equal share keeps all of
/// itself and hands back what it did not need, and the round repeats until only
/// blocks larger than their share are left. Those split what remains.
///
/// Nearly every MCP result is a single text block, where this returns `[cap]`
/// and the whole thing collapses to one `cut`.
fn shares(sizes: &[usize], cap: usize) -> Vec<usize> {
    let mut allowance = vec![0usize; sizes.len()];
    let mut settled = vec![false; sizes.len()];
    let mut left = cap;
    loop {
        let open: Vec<usize> = (0..sizes.len()).filter(|i| !settled[*i]).collect();
        if open.is_empty() {
            break;
        }
        let share = left / open.len();
        let small: Vec<usize> = open
            .iter()
            .copied()
            .filter(|i| sizes[*i] <= share)
            .collect();
        if small.is_empty() {
            // Everything left is over its share, so they divide what remains.
            for i in open {
                allowance[i] = share;
            }
            break;
        }
        for i in small {
            allowance[i] = sizes[i];
            settled[i] = true;
            left -= sizes[i];
        }
    }
    allowance
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
    use taurus_provider::{ToolOutput, ToolResultBlock};

    /// A context with a real budget and somewhere to spill to.
    fn ctx_in(dir: &std::path::Path) -> ToolContext {
        let engine = Arc::new(taurus_tools::PermissionEngine::new(
            dir,
            dir.join(".taurus"),
            Box::new(taurus_tools::AllowAll),
        ));
        ToolContext::new(dir, engine, tokio_util::sync::CancellationToken::new())
            .with_budget(taurus_tools::OutputBudget::for_window(200_000))
            .with_command_output(dir.join("output"))
    }

    fn cap_of(ctx: &ToolContext) -> usize {
        ctx.budget
            .bytes(OUTPUT_SHARE, MIN_OUTPUT_BYTES, MAX_OUTPUT_BYTES)
    }

    #[test]
    fn one_block_is_given_the_whole_allowance() {
        assert_eq!(shares(&[10_000], 500), vec![500]);
    }

    #[test]
    fn a_short_block_beside_a_long_one_is_not_cut() {
        // The reason this is not an even split: halving a 20-byte block to make
        // room in a budget a megabyte block is blowing shortens nothing and
        // puts "bytes omitted" in the middle of a sentence.
        let out = shares(&[20, 1_000_000], 1000);
        assert_eq!(out[0], 20, "the short one keeps all of itself");
        assert_eq!(out[1], 980, "and hands the rest to the long one");
    }

    #[test]
    fn blocks_all_over_their_share_divide_what_is_there() {
        assert_eq!(shares(&[10_000, 10_000], 1000), vec![500, 500]);
    }

    #[test]
    fn everything_fitting_leaves_the_allowance_unspent() {
        // Nothing is cut, so nothing needs an allowance larger than itself.
        assert_eq!(shares(&[10, 20, 30], 1000), vec![10, 20, 30]);
    }

    #[test]
    fn a_result_that_fits_is_passed_through_untouched() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ctx_in(dir.path());
        let out = fit("mcp__x__y", ToolOutput::text("small enough"), &ctx);
        assert_eq!(out.as_text(), Some("small enough"));
    }

    #[test]
    fn a_result_far_too_large_comes_back_bounded() {
        // The finding this closes: a server returning a large page went into
        // the context whole, while every built-in tool beside it was capped.
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ctx_in(dir.path());
        let cap = cap_of(&ctx);
        let huge = "x".repeat(cap * 4);

        let out = fit("mcp__fetch__get", ToolOutput::text(huge), &ctx);
        let text = out.to_text();
        assert!(text.len() < cap * 2, "{} bytes came back", text.len());
        assert!(text.contains("bytes omitted"), "it says how much went");
    }

    #[test]
    fn the_whole_answer_is_written_out_and_the_gap_names_it() {
        // A cut with no file behind it turns "the middle is elsewhere" into
        // "the middle is gone".
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ctx_in(dir.path());
        let cap = cap_of(&ctx);
        let huge = format!("HEAD{}TAIL", "x".repeat(cap * 4));

        let out = fit("mcp__fetch__get", ToolOutput::text(huge.clone()), &ctx);
        let text = out.to_text();
        assert!(text.starts_with("HEAD"), "the head survives");
        assert!(text.ends_with("TAIL"), "and so does the tail");

        let path = text
            .split_once("written to ")
            .and_then(|(_, rest)| rest.split_once(" —"))
            .map(|(path, _)| std::path::PathBuf::from(path))
            .expect("the gap names a file");
        assert_eq!(std::fs::read_to_string(path).unwrap(), huge);
    }

    #[test]
    fn a_picture_beside_too_much_text_is_not_cut_in_half() {
        // Half an image is a broken image, not a smaller one.
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = ctx_in(dir.path());
        let cap = cap_of(&ctx);
        let blocks = ToolOutput::blocks(vec![
            ToolResultBlock::text("x".repeat(cap * 4)),
            ToolResultBlock::image("image/png", "AAAA"),
        ])
        .unwrap();

        let out = fit("mcp__shot__take", blocks, &ctx);
        assert!(
            matches!(
                out.as_slice(),
                [ToolResultBlock::Text { .. }, ToolResultBlock::Image { data, .. }] if data == "AAAA"
            ),
            "the image crosses whole and stays where it was: {:?}",
            out.as_slice()
        );
    }

    #[test]
    fn with_nowhere_to_spill_the_cut_still_happens() {
        // The context a CLI run hands over has no command-output directory.
        // Losing the copy is not a reason to hand the model the whole thing.
        let dir = tempfile::TempDir::new().unwrap();
        let engine = Arc::new(taurus_tools::PermissionEngine::new(
            dir.path(),
            dir.path().join(".taurus"),
            Box::new(taurus_tools::AllowAll),
        ));
        let ctx = ToolContext::new(
            dir.path(),
            engine,
            tokio_util::sync::CancellationToken::new(),
        )
        .with_budget(taurus_tools::OutputBudget::for_window(200_000));
        let cap = cap_of(&ctx);

        let out = fit("mcp__x__y", ToolOutput::text("x".repeat(cap * 4)), &ctx);
        let text = out.to_text();
        assert!(text.len() < cap * 2);
        assert!(text.contains("bytes omitted"));
        assert!(!text.contains("read_file"), "there is no file to name");
    }

    fn connected_status(name: &str) -> ServerStatus {
        ServerStatus {
            name: name.into(),
            description: "a server".into(),
            connected: true,
            tool_count: 3,
            error: None,
            disabled: false,
            tools: vec!["one".into(), "two".into(), "three".into()],
        }
    }

    #[test]
    fn a_server_that_dies_stops_being_listed_as_connected() {
        // The finding this closes: the status map was written once, at connect,
        // so a server that died half an hour ago was still shown green with a
        // tool count while every call against it failed.
        let mut status = BTreeMap::new();
        status.insert("git".to_string(), connected_status("git"));

        assert!(note_gone(&mut status, "git", &"transport closed"));
        let entry = &status["git"];
        assert!(!entry.connected);
        assert_eq!(entry.tool_count, 0);
        assert!(entry.tools.is_empty(), "and stops listing what it offered");
        let said = entry.error.as_deref().unwrap();
        assert!(said.contains("transport closed"), "{said}");
        assert!(said.contains("Reconnect"), "it says what to do: {said}");
    }

    #[test]
    fn a_server_that_never_started_keeps_the_reason_it_did_not() {
        // "uvx is not on the PATH" is the useful sentence. "the transport
        // closed" is only what happened next, and would overwrite it.
        let mut status = BTreeMap::new();
        status.insert(
            "git".to_string(),
            ServerStatus {
                error: Some("uvx is not on the PATH".into()),
                connected: false,
                ..connected_status("git")
            },
        );

        assert!(!note_gone(&mut status, "git", &"transport closed"));
        assert_eq!(
            status["git"].error.as_deref(),
            Some("uvx is not on the PATH")
        );
    }

    #[test]
    fn a_death_reported_twice_is_news_once() {
        // Two tool calls in one turn against the same dead server should not
        // put the same line in the log twice.
        let mut status = BTreeMap::new();
        status.insert("git".to_string(), connected_status("git"));
        assert!(note_gone(&mut status, "git", &"transport closed"));
        assert!(!note_gone(&mut status, "git", &"transport closed"));
    }

    #[test]
    fn a_call_gets_longer_to_answer_than_a_connection_gets_to_open() {
        // They measure different things: a handshake is a round trip, a call is
        // the work. A call deadline at the connect deadline would cut off every
        // tool that does anything.
        assert!(CALL_TIMEOUT > CONNECT_TIMEOUT);
    }

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
