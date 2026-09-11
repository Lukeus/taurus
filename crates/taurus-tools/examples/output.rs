//! What a command's output costs the process that runs it.
//!
//! ```sh
//! cargo build --release -p taurus-tools --example output
//! /usr/bin/time -l target/release/examples/output lines      # macOS; `-v` with GNU time
//! /usr/bin/time -l target/release/examples/output one-line
//! ```
//!
//! Needs no provider, and writes only inside temp directories it makes. Two
//! commands that each print 100 MB — as ordinary eighty-byte lines, and as one
//! line with no newline in it at all — run through `run_command` the way a
//! turn runs them, with a screen attached and somewhere to spill to.
//!
//! What it prints is the wall time, how long the answer the model reads is,
//! and what the screen was sent: how much, in how many messages, and the
//! largest single one, which is one IPC message to the webview. What it costs
//! in memory is the maximum resident set `time` reports, and that is the
//! number to watch: output the model will only ever see the two ends of should
//! not be held whole, and certainly not several times over.
//!
//! Unix only: the commands are `yes`, `head` and `tr`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use taurus_tools::builtin::shell::RunCommand;
use taurus_tools::permission::{AllowAll, PermissionEngine};
use taurus_tools::{Tool, ToolContext, ToolProgress};
use tokio_util::sync::CancellationToken;

/// A screen that counts what it is sent rather than drawing it.
#[derive(Default)]
struct Screen {
    bytes: AtomicUsize,
    messages: AtomicUsize,
    largest: AtomicUsize,
}

#[async_trait::async_trait]
impl ToolProgress for Screen {
    async fn step(&self, text: String) {
        self.bytes.fetch_add(text.len(), Ordering::Relaxed);
        self.messages.fetch_add(1, Ordering::Relaxed);
        self.largest.fetch_max(text.len(), Ordering::Relaxed);
    }
}

#[tokio::main]
async fn main() {
    if cfg!(windows) {
        println!("unix only: the commands are yes, head and tr");
        return;
    }
    let case = std::env::args().nth(1).unwrap_or_else(|| "lines".into());
    let command = match case.as_str() {
        "one-line" => "head -c 100000000 /dev/zero | tr '\\0' x",
        "lines" => {
            "yes 'a line of ordinary build output, about eighty bytes long, as a compiler says' \
             | head -c 100000000"
        }
        other => {
            println!("unknown case {other}: try `lines` or `one-line`");
            return;
        }
    };

    let dir = tempfile::TempDir::new().expect("a temp workspace");
    let root = dir.path().canonicalize().expect("a real path");
    let spills = tempfile::TempDir::new().expect("a temp spill directory");
    let permissions = Arc::new(PermissionEngine::new(
        &root,
        root.join(".taurus"),
        Box::new(AllowAll),
    ));
    let screen = Arc::new(Screen::default());
    let ctx = ToolContext::new(root, permissions, CancellationToken::new())
        .with_progress(screen.clone())
        .with_command_output(spills.path())
        .with_session("measure")
        .with_call_id("output");

    let started = Instant::now();
    let answer = RunCommand
        .execute(
            serde_json::json!({"command": command, "timeout_secs": 600}),
            &ctx,
        )
        .await
        .expect("the command runs")
        .to_text()
        .to_string();
    let took = started.elapsed();

    println!(
        "{case}: {took:.1?}; the model reads {} bytes; the screen was sent {} MB in {} messages, \
         the largest {} bytes",
        answer.len(),
        screen.bytes.load(Ordering::Relaxed) / (1024 * 1024),
        screen.messages.load(Ordering::Relaxed),
        screen.largest.load(Ordering::Relaxed),
    );
}
