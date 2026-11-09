//! `taurus acp` the way an editor drives it: the real binary, over stdin and
//! stdout, against a scripted OpenAI-compatible server and a scratch
//! `TAURUS_HOME`, so nothing on this machine's own config is read or written.

use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Editor {
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    /// Every `session/update` seen so far.
    updates: Vec<Value>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    workspace: std::path::PathBuf,
}

impl Editor {
    async fn start(server: &MockServer) -> Self {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("providers.json"),
            json!([{
                "id": "fake",
                "kind": "open_ai_compatible",
                "base_url": server.uri(),
                "models": [{ "id": "scripted", "native_tools": true, "context_length": 32000 }]
            }])
            .to_string(),
        )
        .unwrap();
        // The once-per-turn "check your work" nudge would ask the scripted
        // model a second time after a write; these tests are about the
        // protocol, not that.
        std::fs::write(
            home.path().join("settings.json"),
            json!({"verify_changes": false}).to_string(),
        )
        .unwrap();
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_taurus"))
            .arg("acp")
            .env("TAURUS_HOME", home.path())
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let path = workspace.path().canonicalize().unwrap();
        Self {
            _child: child,
            stdin,
            lines,
            updates: Vec::new(),
            _dirs: (home, workspace),
            workspace: path,
        }
    }

    async fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    /// The next message that isn't a `session/update`, keeping the updates.
    async fn next(&mut self) -> Value {
        loop {
            let line = tokio::time::timeout(Duration::from_secs(30), self.lines.next_line())
                .await
                .expect("an answer within 30 s")
                .unwrap()
                .expect("the agent is still running");
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|_| panic!("stdout carried something that isn't JSON: {line}"));
            if message["method"] == "session/update" {
                self.updates.push(message["params"]["update"].clone());
                continue;
            }
            return message;
        }
    }

    async fn call(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let answer = self.next().await;
        assert_eq!(answer["id"], id, "{answer}");
        answer
    }

    async fn open(&mut self) -> String {
        let init = self
            .call(
                0,
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
            )
            .await;
        assert_eq!(init["result"]["protocolVersion"], 1, "{init}");
        let cwd = self.workspace.display().to_string();
        let created = self
            .call(1, "session/new", json!({"cwd": cwd, "mcpServers": []}))
            .await;
        created["result"]["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("{created}"))
            .to_string()
    }

    fn said(&self) -> String {
        self.updates
            .iter()
            .filter(|u| u["sessionUpdate"] == "agent_message_chunk")
            .filter_map(|u| u["content"]["text"].as_str())
            .collect()
    }
}

fn sse(chunks: &[Value]) -> ResponseTemplate {
    let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

fn says(text: &str) -> ResponseTemplate {
    sse(&[
        json!({"choices": [{"index": 0, "delta": {"content": text}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}),
    ])
}

#[tokio::test]
async fn a_prompt_streams_its_answer_and_ends_the_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(says("Hello from Taurus."))
        .mount(&server)
        .await;
    let mut editor = Editor::start(&server).await;
    let session = editor.open().await;

    let done = editor
        .call(
            2,
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": "say hello"}]}),
        )
        .await;
    assert_eq!(done["result"]["stopReason"], "end_turn", "{done}");
    assert_eq!(editor.said(), "Hello from Taurus.");
}

#[tokio::test]
async fn a_write_is_asked_about_in_the_editor_and_runs_when_allowed() {
    let server = MockServer::start().await;
    let call = json!({"choices": [{"index": 0, "delta": {"tool_calls": [{
        "index": 0, "id": "call_1", "type": "function",
        "function": {"name": "write_file", "arguments": "{\"path\":\"note.txt\",\"content\":\"from the editor\"}"}
    }]}}]});
    let stop = json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse(&[call, stop]))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(says("Written."))
        .mount(&server)
        .await;
    let mut editor = Editor::start(&server).await;
    let session = editor.open().await;

    editor
        .send(json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "write a note"}]}}))
        .await;

    // The agent asks before writing, with the edit as a diff.
    let asked = editor.next().await;
    assert_eq!(asked["method"], "session/request_permission", "{asked}");
    let tool_call = &asked["params"]["toolCall"];
    assert_eq!(tool_call["kind"], "edit");
    assert_eq!(tool_call["content"][0]["type"], "diff");
    assert_eq!(tool_call["content"][0]["newText"], "from the editor");
    editor
        .send(json!({"jsonrpc": "2.0", "id": asked["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}}}))
        .await;

    let done = editor.next().await;
    assert_eq!(done["id"], 2, "{done}");
    assert_eq!(done["result"]["stopReason"], "end_turn", "{done}");
    assert_eq!(
        std::fs::read_to_string(editor.workspace.join("note.txt")).unwrap(),
        "from the editor"
    );
    let statuses: Vec<&str> = editor
        .updates
        .iter()
        .filter(|u| u["sessionUpdate"] == "tool_call_update")
        .filter_map(|u| u["status"].as_str())
        .collect();
    assert_eq!(statuses, ["completed"]);
    assert_eq!(editor.said(), "Written.");
}

#[tokio::test]
async fn a_denied_write_writes_nothing() {
    let server = MockServer::start().await;
    let call = json!({"choices": [{"index": 0, "delta": {"tool_calls": [{
        "index": 0, "id": "call_1", "type": "function",
        "function": {"name": "write_file", "arguments": "{\"path\":\"note.txt\",\"content\":\"x\"}"}
    }]}}]});
    let stop = json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse(&[call, stop]))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(says("Understood."))
        .mount(&server)
        .await;
    let mut editor = Editor::start(&server).await;
    let session = editor.open().await;
    editor
        .send(json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "write a note"}]}}))
        .await;
    let asked = editor.next().await;
    assert_eq!(asked["method"], "session/request_permission");
    editor
        .send(json!({"jsonrpc": "2.0", "id": asked["id"],
            "result": {"outcome": {"outcome": "cancelled"}}}))
        .await;
    let done = editor.next().await;
    assert_eq!(done["result"]["stopReason"], "end_turn", "{done}");
    assert!(!editor.workspace.join("note.txt").exists());
}

#[tokio::test]
async fn an_unknown_method_and_an_unknown_session_are_errors_not_silence() {
    let server = MockServer::start().await;
    let mut editor = Editor::start(&server).await;
    editor.open().await;
    let unknown = editor.call(7, "session/fly", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");
    let missing = editor
        .call(
            8,
            "session/prompt",
            json!({"sessionId": "nope", "prompt": [{"type": "text", "text": "hi"}]}),
        )
        .await;
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
}
