//! Live smoke test against any OpenAI-compatible endpoint.
//!
//! `cargo run -p taurus-provider-openai --example smoke -- <model> [base_url]`
//!
//! Ollama serves an OpenAI-compatible API at /v1, so this runs against the
//! same local server as the Ollama adapter — a direct comparison of the two
//! code paths over identical hardware.

use taurus_provider::{ChatRequest, Message, Provider, StreamAccumulator, ToolDef};
use taurus_provider_openai::{OpenAiCapabilities, OpenAiProvider};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().unwrap_or_else(|| "llama3.2:latest".into());
    let base_url = args
        .next()
        .unwrap_or_else(|| "http://localhost:11434".into());

    let provider = OpenAiProvider::new(
        "openai-compat",
        base_url,
        std::env::var("OPENAI_API_KEY").ok(),
        OpenAiCapabilities::default(),
    );

    let request = ChatRequest::new(
        &model,
        vec![Message::user(
            "List the contents of /etc. Call the tool, do not guess.",
        )],
    )
    .with_system("You are a terse assistant with tools.")
    .with_tools(vec![ToolDef {
        name: "list_dir".into(),
        description: "List the entries of a directory".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    }]);

    let (tx, mut rx) = mpsc::channel(64);
    let handle =
        tokio::spawn(async move { provider.stream(request, tx, CancellationToken::new()).await });

    let mut acc = StreamAccumulator::new();
    while let Some(event) = rx.recv().await {
        acc.push(event);
    }
    let stop = handle.await??;
    let (message, usage, malformed) = acc.finish();

    println!("model: {model}");
    println!("  stop_reason: {stop:?}");
    println!(
        "  usage: {} in / {} out",
        usage.input_tokens, usage.output_tokens
    );
    if !message.text().trim().is_empty() {
        println!("  text: {}", message.text().trim());
    }
    for (id, name, input) in message.tool_uses() {
        println!("  tool call: {name}({input}) [{id}]");
    }
    if !malformed.is_empty() {
        println!("  malformed: {malformed:?}");
    }
    Ok(())
}
