//! `taurus acp`: Taurus as an agent an editor hosts, over the Agent Client
//! Protocol.
//!
//! Zed, JetBrains IDEs and anything else that speaks ACP start an agent as a
//! child process and talk JSON-RPC to it over stdin and stdout, one message per
//! line. This is that agent. A prompt from the editor runs a turn on the same
//! host the CLI and the app use, so the editor gets Taurus's tools, skills,
//! permission allowlist, undo record and transcripts, and Taurus gets the
//! editor's window.
//!
//! What it maps, and what it doesn't:
//!
//! - **Turns.** `session/prompt` runs one; its text streams back as
//!   `agent_message_chunk`, reasoning as `agent_thought_chunk`, and each tool
//!   call as `tool_call` then `tool_call_update`. `session/cancel` stops it.
//! - **Permission.** A call Taurus would have asked about is asked about in the
//!   editor, through `session/request_permission`, with the file diff when
//!   there is one. "Always" there is Taurus's own "always allow here", kept in
//!   the workspace's `permissions.json` like any other.
//! - **Questions.** `ask_user` has nobody to ask (ACP's way of asking is an
//!   optional capability few editors have), so its questions are answered
//!   "decide for yourself", as in a piped run.
//! - **Sessions.** Each one is an ordinary Taurus conversation, transcript and
//!   all, so `taurus sessions` lists it and the app can open it. `session/load`
//!   isn't offered yet.
//! - **The editor's MCP servers** aren't started; Taurus's own are.
//!
//! The protocol is written out here rather than taken from a crate: it's a
//! handful of methods, and every one of them is a JSON shape mapped onto the
//! host. Stdout carries protocol messages and nothing else, which ACP
//! requires; logs go to stderr.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use taurus_core::{AgentError, Session, UiEvent};
use taurus_host::{sessions, Host, PermissionPromptFactory, SessionLog, TurnRef};
use taurus_provider::{Message, StopReason};
use taurus_tools::{PermissionDecision, PermissionPrompt, PermissionRequest};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::SessionArgs;

/// The protocol version this speaks.
const PROTOCOL_VERSION: i64 = 1;

/// A tool's output is cut to this in what the editor is sent. The model gets
/// the whole of it; the editor's card is for a person to glance at.
const MAX_SHOWN_OUTPUT: usize = 4_000;

/// JSON-RPC's codes for the errors this sends.
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// The way out to the editor: every message goes through here, a line at a
/// time, and requests this makes wait here for their answers.
pub struct Outbox {
    tx: mpsc::UnboundedSender<Value>,
    pending: Mutex<HashMap<i64, oneshot::Sender<Value>>>,
    next: AtomicI64,
}

impl Outbox {
    fn new(tx: mpsc::UnboundedSender<Value>) -> Self {
        Self {
            tx,
            pending: Mutex::default(),
            next: AtomicI64::new(1),
        }
    }

    fn send(&self, message: Value) {
        let _ = self.tx.send(message);
    }

    fn respond(&self, id: Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn fail(&self, id: Value, code: i64, message: impl Into<String>) {
        self.send(json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": code, "message": message.into() }
        }));
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn update(&self, session: &str, update: Value) {
        self.notify(
            "session/update",
            json!({ "sessionId": session, "update": update }),
        );
    }

    /// Sends a request to the editor and waits for its result. `None` when the
    /// editor answered with an error or went away.
    async fn request(&self, method: &str, params: Value) -> Option<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let answer = rx.await.ok()?;
        answer.get("result").cloned()
    }

    /// Hands a response from the editor to the request waiting for it.
    fn answered(&self, message: Value) {
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            return;
        };
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(message);
        }
    }
}

/// Permission prompts that ask the editor.
struct EditorPrompts(Arc<Outbox>);

impl PermissionPromptFactory for EditorPrompts {
    fn create(&self) -> Box<dyn PermissionPrompt> {
        Box::new(EditorPrompt(self.0.clone()))
    }
}

struct EditorPrompt(Arc<Outbox>);

#[async_trait::async_trait]
impl PermissionPrompt for EditorPrompt {
    async fn request(&self, request: PermissionRequest) -> PermissionDecision {
        // A request outside any session has no editor window to appear in.
        let Some(session) = request.session.clone() else {
            return PermissionDecision::Deny;
        };
        let params = permission_params(&session, &request);
        let Some(result) = self.0.request("session/request_permission", params).await else {
            return PermissionDecision::Deny;
        };
        decision(&result)
    }
}

/// What `session/request_permission` is sent for `request`.
fn permission_params(session: &str, request: &PermissionRequest) -> Value {
    let mut options = vec![json!({
        "optionId": "allow-once", "name": "Allow once", "kind": "allow_once"
    })];
    if request.offer_always {
        options.push(json!({
            "optionId": "allow-always",
            "name": format!("Always allow {}", request.always_scope),
            "kind": "allow_always"
        }));
    }
    options.push(json!({
        "optionId": "reject-once", "name": "Deny", "kind": "reject_once"
    }));
    let mut tool_call = json!({
        "toolCallId": request.id,
        "title": request.preview,
        "kind": tool_kind(&request.tool),
        "status": "pending",
        "rawInput": request.input,
    });
    if let Some(content) = edit_content(&request.tool, &request.input) {
        tool_call["content"] = json!([content]);
    }
    if let Some(path) = request.input.get("path").and_then(Value::as_str) {
        tool_call["locations"] = json!([{ "path": path }]);
    }
    json!({ "sessionId": session, "toolCall": tool_call, "options": options })
}

/// The editor's choice, as a decision. Anything but a recognized allow is a
/// deny, a cancelled prompt included.
fn decision(result: &Value) -> PermissionDecision {
    let outcome = &result["outcome"];
    if outcome["outcome"] != "selected" {
        return PermissionDecision::Deny;
    }
    match outcome["optionId"].as_str() {
        Some("allow-once") => PermissionDecision::AllowOnce,
        Some("allow-always") => PermissionDecision::AllowAlways,
        _ => PermissionDecision::Deny,
    }
}

/// An edit as ACP's diff content, so the editor can show it as one. The
/// replacement itself, for `edit_file`: what the model sent is the part that
/// changes, and the editor places it.
fn edit_content(tool: &str, input: &Value) -> Option<Value> {
    let path = input.get("path")?.as_str()?;
    match tool {
        "edit_file" => Some(json!({
            "type": "diff",
            "path": path,
            "oldText": input.get("old_string")?.as_str()?,
            "newText": input.get("new_string")?.as_str()?,
        })),
        "write_file" => Some(json!({
            "type": "diff",
            "path": path,
            "oldText": std::fs::read_to_string(path).ok(),
            "newText": input.get("content")?.as_str()?,
        })),
        _ => None,
    }
}

/// ACP's kind for one of Taurus's tools, which decides the icon an editor
/// draws. A tool it doesn't know is `other`.
fn tool_kind(tool: &str) -> &'static str {
    match tool {
        "read_file" | "list_dir" | "read_note" | "read_terminal" => "read",
        "glob" | "grep" | "search_code" => "search",
        "write_file" | "edit_file" | "write_note" | "apply_delegate" => "edit",
        "run_command" | "check_command" | "stop_command" | "run_skill_script" => "execute",
        "fetch_url" | "web_search" => "fetch",
        "update_plan" => "think",
        _ => "other",
    }
}

/// What the editor is sent for one of the turn's events. `None` for the ones
/// it has no place for.
fn update_for(event: &UiEvent, names: &mut HashMap<String, String>) -> Option<Value> {
    match event {
        UiEvent::TextDelta { text } => Some(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": text }
        })),
        UiEvent::ThinkingDelta { text } => Some(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": text }
        })),
        UiEvent::ToolCallStarted {
            id, name, preview, ..
        } => {
            names.insert(id.clone(), name.clone());
            Some(json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "title": preview,
                "kind": tool_kind(name),
                "status": "in_progress",
            }))
        }
        UiEvent::ToolProgress { id, label } => Some(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "title": label,
        })),
        UiEvent::ToolCallFinished { id, ok, output, .. } => Some(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": if *ok { "completed" } else { "failed" },
            "content": [{ "type": "content", "content": { "type": "text", "text": shown(output) } }],
        })),
        // Said in the conversation, because a failure the editor never shows
        // is one nobody can act on.
        UiEvent::Error { message } => Some(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": format!("\n\n> {message}\n") }
        })),
        _ => None,
    }
}

fn shown(output: &str) -> String {
    if output.len() <= MAX_SHOWN_OUTPUT {
        return output.to_string();
    }
    let mut end = MAX_SHOWN_OUTPUT;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… cut for display", &output[..end])
}

/// What the editor's prompt says, as one message. Text as it is; an embedded
/// file as a fenced block naming it; a link to a file as its path.
fn prompt_text(blocks: &[Value]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = block["text"].as_str() {
                    parts.push(text.to_string());
                }
            }
            Some("resource") => {
                let resource = &block["resource"];
                let uri = resource["uri"].as_str().unwrap_or("");
                if let Some(text) = resource["text"].as_str() {
                    parts.push(format!("{}:\n```\n{text}\n```", display_uri(uri)));
                }
            }
            Some("resource_link") => {
                if let Some(uri) = block["uri"].as_str() {
                    parts.push(display_uri(uri));
                }
            }
            _ => {}
        }
    }
    parts.join("\n\n")
}

fn display_uri(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

/// ACP's stop reason for how a turn ended, or the error to answer with.
fn stop_reason(
    outcome: &Result<taurus_core::TurnOutcome, AgentError>,
) -> Result<&'static str, String> {
    match outcome {
        Ok(done) => Ok(match done.stop_reason {
            StopReason::MaxTokens => "max_tokens",
            StopReason::Canceled => "cancelled",
            _ => "end_turn",
        }),
        Err(AgentError::IterationLimit(_)) => Ok("max_turn_requests"),
        Err(e) => Err(e.to_string()),
    }
}

/// One conversation the editor has open.
struct Conversation {
    state: tokio::sync::Mutex<(Session, SessionLog)>,
    provider: String,
    model: String,
    /// The turn running now, if one is, so `session/cancel` can stop it.
    running: Mutex<Option<CancellationToken>>,
}

struct Server {
    host: Arc<Host>,
    out: Arc<Outbox>,
    args: SessionArgs,
    sessions: Mutex<HashMap<String, Arc<Conversation>>>,
}

/// Runs the agent until the editor closes stdin.
pub async fn serve(args: SessionArgs) -> Result<std::process::ExitCode, String> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let out = Arc::new(Outbox::new(tx));
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = rx.recv().await {
            // `to_string` never writes a raw newline, which is what ACP's
            // framing needs: one message per line.
            let mut line = message.to_string();
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    let workspace = match &args.workspace {
        Some(path) => path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?,
        None => Host::default_workspace(),
    };
    let host = Arc::new(Host::new(
        workspace,
        Arc::new(EditorPrompts(out.clone())),
        Arc::new(taurus_tools::Unattended),
        Arc::new(taurus_skills::proposal::CollectingSink::default()),
        Arc::new(taurus_agents::proposal::CollectingSink::default()),
    ));
    host.reload().await;

    let server = Arc::new(Server {
        host,
        out: out.clone(),
        args,
        sessions: Mutex::default(),
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            eprintln!("taurus acp: a line from the editor wasn't JSON");
            continue;
        };
        let server = server.clone();
        // Each message on its own task: a prompt runs for minutes, and the
        // cancel and permission answers for it arrive while it does.
        tokio::spawn(async move { server.handle(message).await });
    }
    server.host.stop_background();
    drop(server);
    drop(out);
    let _ = writer.await;
    Ok(std::process::ExitCode::SUCCESS)
}

impl Server {
    async fn handle(&self, message: Value) {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, id) {
            (None, Some(_)) => self.out.answered(message),
            (Some("session/cancel"), None) => self.cancel(&params),
            (Some(method), Some(id)) => match self.call(method, params).await {
                Ok(result) => self.out.respond(id, result),
                Err((code, why)) => self.out.fail(id, code, why),
            },
            _ => {}
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => Ok(self.initialize(&params)),
            "authenticate" => Ok(json!({})),
            "session/new" => self.new_session(&params).await,
            "session/prompt" => self.prompt(&params).await,
            other => Err((METHOD_NOT_FOUND, format!("{other} isn't supported"))),
        }
    }

    fn initialize(&self, params: &Value) -> Value {
        let asked = params["protocolVersion"]
            .as_i64()
            .unwrap_or(PROTOCOL_VERSION);
        json!({
            "protocolVersion": asked.min(PROTOCOL_VERSION),
            "agentCapabilities": {
                "loadSession": false,
                "promptCapabilities": { "image": false, "audio": false, "embeddedContext": true },
                "mcpCapabilities": { "http": false, "sse": false }
            },
            "agentInfo": {
                "name": "taurus",
                "title": "Taurus",
                "version": env!("CARGO_PKG_VERSION")
            },
            "authMethods": []
        })
    }

    async fn new_session(&self, params: &Value) -> Result<Value, (i64, String)> {
        let cwd = params["cwd"]
            .as_str()
            .map(PathBuf::from)
            .ok_or((INVALID_PARAMS, "session/new needs a cwd".to_string()))?;
        self.enter(&cwd).await?;
        let (provider, model) = self
            .host
            .resolve_model(self.args.provider.as_deref(), self.args.model.as_deref())
            .await
            .map_err(|e| (INTERNAL_ERROR, e))?;
        let session = Session::new(&model);
        let workspace = self.host.workspace().await;
        let log = SessionLog::create(&session, &workspace, self.host.branch().await);
        let id = session.id.clone();
        let conversation = Arc::new(Conversation {
            state: tokio::sync::Mutex::new((session, log)),
            provider,
            model,
            running: Mutex::new(None),
        });
        self.sessions
            .lock()
            .unwrap()
            .insert(id.clone(), conversation);
        Ok(json!({ "sessionId": id }))
    }

    /// Makes `cwd` the host's workspace. One host has one workspace, so a
    /// second folder while a turn runs in the first is refused rather than
    /// switched under it.
    async fn enter(&self, cwd: &Path) -> Result<(), (i64, String)> {
        let canonical = cwd
            .canonicalize()
            .map_err(|e| (INVALID_PARAMS, format!("{}: {e}", cwd.display())))?;
        if self.host.workspace().await == canonical {
            return Ok(());
        }
        let busy = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .any(|c| c.running.lock().unwrap().is_some());
        if busy {
            return Err((
                INVALID_PARAMS,
                "a turn is running in another folder; open this one once it ends".into(),
            ));
        }
        self.host
            .set_workspace(&canonical)
            .await
            .map_err(|e| (INVALID_PARAMS, e))?;
        Ok(())
    }

    fn conversation(&self, params: &Value) -> Result<(String, Arc<Conversation>), (i64, String)> {
        let id = params["sessionId"]
            .as_str()
            .ok_or((INVALID_PARAMS, "no sessionId".to_string()))?;
        let found = self.sessions.lock().unwrap().get(id).cloned();
        found
            .map(|c| (id.to_string(), c))
            .ok_or((INVALID_PARAMS, format!("no session {id}")))
    }

    fn cancel(&self, params: &Value) {
        if let Ok((_, conversation)) = self.conversation(params) {
            if let Some(token) = conversation.running.lock().unwrap().as_ref() {
                token.cancel();
            }
        }
    }

    async fn prompt(&self, params: &Value) -> Result<Value, (i64, String)> {
        let (id, conversation) = self.conversation(params)?;
        let blocks = params["prompt"].as_array().cloned().unwrap_or_default();
        let task = prompt_text(&blocks);
        if task.trim().is_empty() {
            return Err((INVALID_PARAMS, "the prompt has no text in it".into()));
        }

        // One turn at a time in a conversation: the second waits here.
        let mut state = conversation.state.lock().await;
        let cancel = CancellationToken::new();
        *conversation.running.lock().unwrap() = Some(cancel.clone());
        let outcome = self
            .turn(&id, &conversation, &mut state, &task, cancel)
            .await;
        *conversation.running.lock().unwrap() = None;
        drop(state);

        match outcome {
            Ok(reason) => Ok(json!({ "stopReason": reason })),
            Err(why) => Err((INTERNAL_ERROR, why)),
        }
    }

    async fn turn(
        &self,
        id: &str,
        conversation: &Conversation,
        state: &mut (Session, SessionLog),
        task: &str,
        cancel: CancellationToken,
    ) -> Result<&'static str, String> {
        let (session, log) = state;
        let host = &self.host;
        host.remember_session(&conversation.provider, &conversation.model)
            .await;
        host.ensure_on_disk(&session.id).await?;
        // A `/name` runs that skill or agent, as it does everywhere else.
        let prompt = match host.expand_command(task).await {
            Some(Ok(invocation)) => invocation.prompt,
            Some(Err(e)) => {
                self.out.update(
                    id,
                    json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": e.to_string() }
                    }),
                );
                return Ok("end_turn");
            }
            None => task.to_string(),
        };
        let provider = host.provider(&conversation.provider).await?;
        let agent = host
            .build_agent(
                provider,
                &conversation.model,
                cancel,
                TurnRef {
                    session_id: &session.id,
                    prompt: task,
                    unattended: None,
                },
            )
            .await;

        let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
        let out = self.out.clone();
        let session_id = id.to_string();
        let forwarder = tokio::spawn(async move {
            let mut names = HashMap::new();
            while let Some(event) = rx.recv().await {
                if let Some(update) = update_for(&event, &mut names) {
                    out.update(&session_id, update);
                }
            }
        });

        let shared = Arc::new(tokio::sync::Mutex::new(std::mem::replace(
            log,
            SessionLog::disabled(),
        )));
        let agent = agent.with_recorder(Arc::new(sessions::SharedLog(shared.clone())));
        let outcome = agent.run_turn(session, Message::user(prompt), tx).await;
        drop(agent);
        let _ = forwarder.await;
        *log = Arc::try_unwrap(shared)
            .map_err(|_| "the transcript was still in use after the turn".to_string())?
            .into_inner();
        stop_reason(&outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(tool: &str, input: Value, offer_always: bool) -> PermissionRequest {
        serde_json::from_value(json!({
            "id": "call_1",
            "tool": tool,
            "effect": "write",
            "preview": "Edit src/a.rs",
            "always_scope": "edit_file in this workspace",
            "offer_always": offer_always,
            "input": input,
            "session": "s1",
        }))
        .expect("a request this build reads")
    }

    #[test]
    fn a_permission_request_carries_the_edit_as_a_diff() {
        let req = request(
            "edit_file",
            json!({"path": "/w/src/a.rs", "old_string": "a", "new_string": "b"}),
            true,
        );
        let params = permission_params("s1", &req);
        assert_eq!(params["sessionId"], "s1");
        let call = &params["toolCall"];
        assert_eq!(call["toolCallId"], "call_1");
        assert_eq!(call["kind"], "edit");
        assert_eq!(
            call["content"][0],
            json!({"type": "diff", "path": "/w/src/a.rs", "oldText": "a", "newText": "b"})
        );
        assert_eq!(call["locations"][0]["path"], "/w/src/a.rs");
        let kinds: Vec<&str> = params["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["allow_once", "allow_always", "reject_once"]);
    }

    #[test]
    fn always_is_offered_only_where_taurus_offers_it() {
        let req = request("run_command", json!({"command": "ls"}), false);
        let params = permission_params("s1", &req);
        assert_eq!(params["options"].as_array().unwrap().len(), 2);
        assert_eq!(params["toolCall"]["kind"], "execute");
    }

    #[test]
    fn the_editor_s_choice_becomes_a_decision_and_anything_else_denies() {
        let pick = |id: &str| json!({"outcome": {"outcome": "selected", "optionId": id}});
        assert_eq!(decision(&pick("allow-once")), PermissionDecision::AllowOnce);
        assert_eq!(
            decision(&pick("allow-always")),
            PermissionDecision::AllowAlways
        );
        assert_eq!(decision(&pick("reject-once")), PermissionDecision::Deny);
        assert_eq!(decision(&pick("made-up")), PermissionDecision::Deny);
        assert_eq!(
            decision(&json!({"outcome": {"outcome": "cancelled"}})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn a_prompt_s_blocks_become_one_message() {
        let text = prompt_text(&[
            json!({"type": "text", "text": "Explain this:"}),
            json!({"type": "resource", "resource": {"uri": "file:///w/a.rs", "text": "fn a() {}"}}),
            json!({"type": "resource_link", "uri": "file:///w/b.rs", "name": "b.rs"}),
            json!({"type": "image", "data": "...", "mimeType": "image/png"}),
        ]);
        assert_eq!(
            text,
            "Explain this:\n\n/w/a.rs:\n```\nfn a() {}\n```\n\n/w/b.rs"
        );
    }

    #[test]
    fn a_turn_s_events_become_session_updates() {
        let mut names = HashMap::new();
        let started = update_for(
            &UiEvent::ToolCallStarted {
                id: "t1".into(),
                name: "grep".into(),
                preview: "Search for fn main".into(),
                view: None,
            },
            &mut names,
        )
        .unwrap();
        assert_eq!(started["sessionUpdate"], "tool_call");
        assert_eq!(started["kind"], "search");
        assert_eq!(started["status"], "in_progress");

        let finished = update_for(
            &UiEvent::ToolCallFinished {
                id: "t1".into(),
                ok: false,
                output: "x".repeat(MAX_SHOWN_OUTPUT + 10),
                images: vec![],
            },
            &mut names,
        )
        .unwrap();
        assert_eq!(finished["status"], "failed");
        let shown = finished["content"][0]["content"]["text"].as_str().unwrap();
        assert!(shown.ends_with("cut for display"));

        let text = update_for(&UiEvent::TextDelta { text: "hi".into() }, &mut names).unwrap();
        assert_eq!(text["sessionUpdate"], "agent_message_chunk");
        assert_eq!(text["content"]["text"], "hi");
        assert!(update_for(&UiEvent::IterationStarted { iteration: 1 }, &mut names).is_none());
    }

    #[test]
    fn how_a_turn_ended_is_an_acp_stop_reason() {
        let done = |stop_reason| {
            Ok(taurus_core::TurnOutcome {
                stop_reason,
                usage: Default::default(),
                iterations: 1,
            })
        };
        assert_eq!(stop_reason(&done(StopReason::EndTurn)), Ok("end_turn"));
        assert_eq!(stop_reason(&done(StopReason::Canceled)), Ok("cancelled"));
        assert_eq!(stop_reason(&done(StopReason::MaxTokens)), Ok("max_tokens"));
        assert_eq!(
            stop_reason(&Err(AgentError::IterationLimit(25))),
            Ok("max_turn_requests")
        );
    }
}
