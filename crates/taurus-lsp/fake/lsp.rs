//! A language server for the tests: one error per line containing `BROKEN`.
//!
//! Before it publishes, it asks the client for its configuration, the way real
//! servers do, so a client that leaves that request unanswered never sees a
//! diagnostic here either.
//!
//! With `--slow`, it answers 700 ms late. Its publishes carry no version, like typescript-language-server's, and a
//! closed file's empty publish is sent late: just after the next file is
//! opened or changed, where a client that can't tell the two apart will read
//! it as that file's answer.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn read(input: &mut impl BufRead) -> Option<Value> {
    let mut length = 0;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(message: &Value) {
    let body = message.to_string();
    let mut out = std::io::stdout().lock();
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    out.flush().unwrap();
}

fn main() {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    // `--slow` answers after a client's settle window has closed, the way a
    // server's semantic pass does on a real project.
    let slow = std::env::args().any(|a| a == "--slow");
    // The document waiting on the configuration reply.
    let mut waiting: Option<(String, String)> = None;
    // A closed file's empty publish, held until the next open or change.
    let mut cleared: Option<String> = None;
    while let Some(message) = read(&mut input) {
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        let params = &message["params"];
        match method {
            "initialize" => {
                send(&json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {}}}))
            }
            "shutdown" => send(&json!({"jsonrpc": "2.0", "id": id, "result": null})),
            "exit" => return,
            "textDocument/didClose" => {
                cleared = params["textDocument"]["uri"].as_str().map(str::to_string);
            }
            "textDocument/didOpen" | "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap().to_string();
                let text = if method == "textDocument/didOpen" {
                    &params["textDocument"]["text"]
                } else {
                    &params["contentChanges"][0]["text"]
                };
                if let Some(closed) = cleared.take() {
                    send(
                        &json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                        "params": {"uri": closed, "diagnostics": []}}),
                    );
                }
                waiting = Some((uri, text.as_str().unwrap().to_string()));
                send(
                    &json!({"jsonrpc": "2.0", "id": 900, "method": "workspace/configuration",
                    "params": {"items": [{"section": "fake"}]}}),
                );
            }
            // The client's answer to the configuration request.
            "" if id == Some(json!(900)) => {
                let Some((uri, text)) = waiting.take() else {
                    continue;
                };
                let diagnostics: Vec<Value> = text
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| line.contains("BROKEN"))
                    .map(|(n, _)| json!({
                        "range": {"start": {"line": n, "character": 0}, "end": {"line": n, "character": 6}},
                        "severity": 1, "message": "this line is broken", "code": "F1"
                    }))
                    .collect();
                if slow {
                    std::thread::sleep(std::time::Duration::from_millis(700));
                }
                send(
                    &json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                    "params": {"uri": uri, "diagnostics": diagnostics}}),
                );
            }
            _ => {}
        }
    }
}
