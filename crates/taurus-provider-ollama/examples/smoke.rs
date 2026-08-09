//! Live smoke test against a running Ollama server.
//!
//! `cargo run -p taurus-provider-ollama --example smoke -- <model>`
//!
//! Exercises the whole adapter against a real model: capability probe, tool
//! definition, streaming, and reassembly. Point it at a model with native tool
//! support and one without to confirm both paths behave identically.

use taurus_provider::{ChatRequest, Message, Provider, StreamAccumulator, StreamEvent, ToolDef};
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args().nth(1).unwrap_or_else(|| "llama3.2".into());
    let provider = OllamaProvider::new(DEFAULT_BASE_URL);

    let caps = provider.capabilities(&model).await?;
    println!("model: {model}");
    println!(
        "  native_tools={} thinking={} vision={} ctx={}",
        caps.native_tools, caps.thinking, caps.vision, caps.context_length
    );
    println!(
        "  path: {}",
        if caps.native_tools {
            "native tool calling"
        } else {
            "prompted fallback"
        }
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
            "properties": { "path": { "type": "string", "description": "Absolute path" } },
            "required": ["path"]
        }),
    }]);

    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let handle = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

    let mut acc = StreamAccumulator::new();
    while let Some(event) = rx.recv().await {
        if let StreamEvent::TextDelta { text } = &event {
            print!("{text}");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
        acc.push(event);
    }
    println!();

    let stop = handle.await??;
    let (message, usage, malformed) = acc.finish();

    println!("  stop_reason: {stop:?}");
    println!(
        "  usage: {} in / {} out",
        usage.input_tokens, usage.output_tokens
    );
    for (id, name, input) in message.tool_uses() {
        println!("  tool call: {name}({input}) [{id}]");
    }
    if !malformed.is_empty() {
        println!("  malformed tool inputs: {malformed:?}");
    }
    Ok(())
}
