//! A stand-in MCP server for tests, played by the test binary itself.
//!
//! Keeping a server running across a reload is only a claim about a server that
//! is running, and a test cannot make one from a program that may not be
//! installed — `npx` is not on a CI runner's PATH, and a shell script speaking
//! JSON-RPC does not run on Windows. The test binary is the one program every
//! platform is guaranteed to have. Started again with a filter naming one
//! ignored test, and a variable that test recognizes, it answers `initialize`,
//! `tools/list` and `tools/call` over stdio until its pipe closes.
//!
//! Not compiled into anything but tests: this crate's own under `cfg(test)`,
//! and another crate's through the `testing` feature it names in its
//! dev-dependencies.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::ServerConfig;

/// Set on the process that is to serve, holding the tools it offers.
///
/// Unset in an ordinary test run, which is what makes [`serve`] return at once
/// there rather than wait on a stdin nobody writes to.
pub const STAND_IN_ENV: &str = "TAURUS_MCP_STAND_IN";

/// An entry that starts the stand-in, offering `tools`.
///
/// `test` is the full path of the ignored test in the calling binary whose body
/// is [`serve`], as `cargo test -- --list` prints it.
pub fn stand_in(test: &str, tools: &[&str]) -> ServerConfig {
    let exe = std::env::current_exe().expect("the test binary knows where it is");
    ServerConfig::Stdio {
        command: exe.display().to_string(),
        args: ["--exact", test, "--ignored", "--quiet", "--test-threads=1"]
            .map(String::from)
            .to_vec(),
        env: BTreeMap::from([(STAND_IN_ENV.to_string(), tools.join(","))]),
        disabled: false,
    }
}

/// Serves MCP over stdio when this process was started by [`stand_in`].
///
/// Every tool answers with this process's id, which is how a test tells a
/// server that was kept from one that was started again under the same name.
pub fn serve() {
    let Ok(tools) = std::env::var(STAND_IN_ENV) else {
        return;
    };
    let listed: Vec<Value> = tools
        .split(',')
        .filter(|tool| !tool.is_empty())
        .map(|tool| json!({"name": tool, "inputSchema": {"type": "object"}}))
        .collect();

    // Written to the handle rather than through `println!`, which the test
    // harness captures.
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // A notification wants no answer.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let answer = match message["method"].as_str().unwrap_or_default() {
            "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": message["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "stand-in", "version": "0"},
            }}),
            "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": listed}}),
            "tools/call" => json!({"jsonrpc": "2.0", "id": id, "result": {
                "content": [{"type": "text", "text": std::process::id().to_string()}],
            }}),
            "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            other => json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32601,
                "message": format!("the stand-in does not answer {other}"),
            }}),
        };
        // Led by a newline, because the harness may have left a line of its
        // own unfinished on this stdout. The client skips a blank line; it
        // would not recover a message glued to the end of someone else's.
        if writeln!(out, "\n{answer}")
            .and_then(|()| out.flush())
            .is_err()
        {
            break;
        }
    }
}
