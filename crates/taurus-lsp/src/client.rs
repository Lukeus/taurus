//! One running language server, spoken to over its stdin and stdout.
//!
//! Only the slice of the protocol diagnostics need: initialize, open a file,
//! save it, read what the server publishes about it, close it. The requests a
//! server sends the other way (its configuration, a progress token, a
//! capability it wants to register) are answered with the emptiest reply that
//! keeps it going, because a server that waits on an unanswered request stops
//! publishing anything.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use taurus_process::Tree;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{oneshot, watch};
use tracing::{debug, warn};

use crate::{uri, Diagnostic, Severity};

/// How long a server has to answer `initialize`. Longer than any check waits,
/// because a server is started in the background and a slow start costs only
/// the edits made while it's starting.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// What a server last published about one file.
struct Publish {
    /// Which publish this was, counted across every file, so a check can tell
    /// one that arrived after it opened the file from one that was already
    /// there.
    seq: u64,
    version: Option<i64>,
    diagnostics: Vec<Diagnostic>,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub(crate) struct Server {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Pending,
    published: Arc<Mutex<HashMap<String, Publish>>>,
    /// Bumped on every publish. A check waits on it.
    seq: watch::Receiver<u64>,
    alive: Arc<AtomicBool>,
    next_id: AtomicI64,
    next_version: AtomicI64,
    /// One check at a time: a file can't be open twice, and two checks of
    /// different files would each wait out the other's publishes.
    checking: tokio::sync::Mutex<()>,
    tree: tokio::sync::Mutex<Tree>,
}

impl Server {
    /// Starts `command` in `root` and initializes it.
    pub(crate) async fn start(
        mut command: tokio::process::Command,
        root: &Path,
        options: Option<Value>,
    ) -> Result<Self, String> {
        command
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        taurus_process::no_console(&mut command);
        taurus_process::path::apply(&mut command);
        let mut tree = Tree::spawn(command).map_err(|e| e.to_string())?;
        let stdin = tree.take_stdin().ok_or("no stdin")?;
        let stdout = tree.take_stdout().ok_or("no stdout")?;
        if let Some(mut stderr) = tree.take_stderr() {
            // Drained, or a chatty server fills the pipe and stops.
            tokio::spawn(async move {
                let mut sink = Vec::new();
                let _ = stderr.read_to_end(&mut sink).await;
            });
        }

        let stdin = Arc::new(tokio::sync::Mutex::new(stdin));
        let pending: Pending = Arc::default();
        let published: Arc<Mutex<HashMap<String, Publish>>> = Arc::default();
        let (seq_tx, seq) = watch::channel(0u64);
        let alive = Arc::new(AtomicBool::new(true));
        let root_uri = uri::from_path(root);
        tokio::spawn(read_loop(
            stdout,
            stdin.clone(),
            pending.clone(),
            published.clone(),
            seq_tx,
            alive.clone(),
            root_uri.clone(),
        ));

        let server = Self {
            stdin,
            pending,
            published,
            seq,
            alive,
            next_id: AtomicI64::new(1),
            next_version: AtomicI64::new(1),
            checking: tokio::sync::Mutex::new(()),
            tree: tokio::sync::Mutex::new(tree),
        };
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "taurus" },
            "rootUri": root_uri,
            "rootPath": root.to_string_lossy(),
            "workspaceFolders": [{ "uri": root_uri, "name": name }],
            "capabilities": {
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "publishDiagnostics": { "versionSupport": true }
                },
                "workspace": { "configuration": true, "workspaceFolders": true }
            }
        });
        if let Some(options) = options {
            params["initializationOptions"] = options;
        }
        match tokio::time::timeout(INITIALIZE_TIMEOUT, server.request("initialize", params)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("didn't answer initialize within a minute".into()),
        }
        server.notify("initialized", json!({})).await?;
        Ok(server)
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Opens `path` with `text`, saves it, and returns what the server
    /// publishes about it within `budget`. `None` when it published nothing
    /// in time.
    ///
    /// After the first publish, waits `settle` for more: several servers
    /// publish in stages, syntax first and then types, and the later one is
    /// the one worth having. The file is closed again afterwards, so the
    /// server goes back to reading it from disk, where a command may change
    /// it next.
    pub(crate) async fn check(
        &self,
        path: &Path,
        language_id: &str,
        text: &str,
        budget: Duration,
        settle: Duration,
    ) -> Result<Option<Vec<Diagnostic>>, String> {
        let _one = self.checking.lock().await;
        let deadline = Instant::now() + budget;
        let file_uri = uri::from_path(path);
        let key = uri::key(&file_uri);
        let version = self.next_version.fetch_add(1, Ordering::SeqCst);
        let mut seq = self.seq.clone();
        let since = *seq.borrow_and_update();

        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": file_uri, "languageId": language_id, "version": version, "text": text
            }}),
        )
        .await?;
        self.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": file_uri } }),
        )
        .await?;

        let mut found: Option<Vec<Diagnostic>> = None;
        let mut until = deadline;
        loop {
            {
                let published = self.published.lock().unwrap();
                if let Some(p) = published.get(&key) {
                    let current = p.version.is_none_or(|v| v >= version);
                    if p.seq > since && current {
                        if found.is_none() {
                            until = deadline.min(Instant::now() + settle);
                        }
                        found = Some(p.diagnostics.clone());
                    }
                }
            }
            if !self.is_alive() {
                break;
            }
            match tokio::time::timeout_at(until.into(), seq.changed()).await {
                Ok(Ok(())) => continue,
                // The reader ended, or time ran out.
                Ok(Err(_)) | Err(_) => break,
            }
        }

        let _ = self
            .notify(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": file_uri } }),
            )
            .await;
        Ok(found)
    }

    /// Asks the server to exit, then ends whatever is left of it.
    pub(crate) async fn shutdown(&self) {
        if self.is_alive() {
            let polite = async {
                let _ = self.request("shutdown", Value::Null).await;
                let _ = self.notify("exit", Value::Null).await;
            };
            let _ = tokio::time::timeout(Duration::from_secs(2), polite).await;
        }
        let _ = self.tree.lock().await.end().await;
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let sent = write(
            &self.stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await;
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        rx.await.map_err(|_| "the server exited".to_string())?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        write(
            &self.stdin,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
        .await
    }
}

async fn write(stdin: &tokio::sync::Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let body = message.to_string();
    let mut stdin = stdin.lock().await;
    let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
    stdin
        .write_all(frame.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stdin.flush().await.map_err(|e| e.to_string())
}

/// Reads one framed message. `None` at the end of the stream or on a frame
/// that can't be read, either of which ends the server as far as this client
/// is concerned.
async fn read_message(reader: &mut BufReader<ChildStdout>) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

async fn read_loop(
    stdout: ChildStdout,
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Pending,
    published: Arc<Mutex<HashMap<String, Publish>>>,
    seq: watch::Sender<u64>,
    alive: Arc<AtomicBool>,
    root_uri: String,
) {
    let mut reader = BufReader::new(stdout);
    let mut count = 0u64;
    while let Some(message) = read_message(&mut reader).await {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match (method, id) {
            // A response to one of ours.
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { continue };
                let Some(tx) = pending.lock().unwrap().remove(&id) else {
                    continue;
                };
                let outcome = match message.get("error") {
                    Some(error) => Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the server refused")
                        .to_string()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(outcome);
            }
            // A request of the server's.
            (Some(method), Some(id)) => {
                let reply = answer(method, message.get("params"), &root_uri);
                let response = match reply {
                    Some(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    None => json!({ "jsonrpc": "2.0", "id": id, "error": {
                        "code": -32601, "message": "not supported by this client"
                    }}),
                };
                if write(&stdin, &response).await.is_err() {
                    break;
                }
            }
            (Some("textDocument/publishDiagnostics"), None) => {
                let Some(params) = message.get("params") else {
                    continue;
                };
                let Some(file) = params.get("uri").and_then(Value::as_str) else {
                    continue;
                };
                count += 1;
                let diagnostics = params
                    .get("diagnostics")
                    .and_then(Value::as_array)
                    .map(|all| all.iter().filter_map(diagnostic).collect())
                    .unwrap_or_default();
                published.lock().unwrap().insert(
                    uri::key(file),
                    Publish {
                        seq: count,
                        version: params.get("version").and_then(Value::as_i64),
                        diagnostics,
                    },
                );
                let _ = seq.send(count);
            }
            (Some(other), None) => debug!(method = other, "language server notification"),
            (None, None) => {}
        }
    }
    alive.store(false, Ordering::SeqCst);
    // Every request still waiting gets its answer: the server's gone.
    pending.lock().unwrap().clear();
    warn!("language server exited");
}

/// The reply to a request a server sends this client, or `None` to refuse it.
fn answer(method: &str, params: Option<&Value>, root_uri: &str) -> Option<Value> {
    match method {
        // One `null` per item asked about: "use your defaults".
        "workspace/configuration" => {
            let items = params
                .and_then(|p| p.get("items"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Some(Value::Array(vec![Value::Null; items]))
        }
        "workspace/workspaceFolders" => Some(json!([{ "uri": root_uri, "name": "workspace" }])),
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "window/showMessageRequest"
        | "workspace/diagnostic/refresh"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/codeLens/refresh" => Some(Value::Null),
        _ => None,
    }
}

fn diagnostic(value: &Value) -> Option<Diagnostic> {
    let start = value.get("range")?.get("start")?;
    let code = match value.get("code") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    Some(Diagnostic {
        line: start.get("line")?.as_u64()? as u32 + 1,
        column: start.get("character")?.as_u64()? as u32 + 1,
        severity: match value.get("severity").and_then(Value::as_u64) {
            Some(1) => Severity::Error,
            Some(2) => Severity::Warning,
            Some(3) => Severity::Information,
            Some(4) => Severity::Hint,
            // The spec leaves a missing severity to the client. A server that
            // didn't say is usually reporting the thing that broke the build.
            _ => Severity::Error,
        },
        message: value.get("message")?.as_str()?.to_string(),
        source: value
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
        code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_answered_item_for_item() {
        let params = json!({ "items": [{}, {}, {}] });
        assert_eq!(
            answer("workspace/configuration", Some(&params), "file:///x"),
            Some(json!([null, null, null]))
        );
        assert_eq!(answer("something/else", None, "file:///x"), None);
    }

    #[test]
    fn a_published_diagnostic_is_one_based() {
        let d = diagnostic(&json!({
            "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 3 } },
            "severity": 2, "message": "unused", "code": 6133, "source": "ts"
        }))
        .unwrap();
        assert_eq!((d.line, d.column), (5, 1));
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.code.as_deref(), Some("6133"));
    }
}
