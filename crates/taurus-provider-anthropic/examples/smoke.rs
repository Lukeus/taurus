//! Live smoke test against the Anthropic Messages API.
//!
//! `ANTHROPIC_API_KEY=… cargo run -p taurus-provider-anthropic --example smoke -- <model>`
//!
//! Exercises the two things unit tests cannot: that a real key and a real
//! model produce a tool call this adapter reassembles, and that the models
//! endpoint reports a context window rather than needing one configured.

use taurus_provider::{ChatRequest, Message, Provider, StreamAccumulator, ToolDef};
use taurus_provider_anthropic::{AnthropicProvider, DEFAULT_BASE_URL};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().unwrap_or_else(|| "claude-opus-5".into());
    let base_url = args.next().unwrap_or_else(|| DEFAULT_BASE_URL.into());

    let key = std::env::var("ANTHROPIC_API_KEY").ok();
    if key.is_none() {
        eprintln!("warning: ANTHROPIC_API_KEY is unset; expect a credentials error");
    }
    let provider = AnthropicProvider::new("anthropic", base_url, key);

    // Probed, not configured. This is the half of the adapter that has no
    // equivalent on an OpenAI-compatible endpoint.
    match provider.capabilities(&model).await {
        Ok(caps) => println!(
            "capabilities: {} token window, vision {}, thinking {}",
            caps.context_length, caps.vision, caps.thinking
        ),
        Err(e) => println!("capabilities: unavailable ({e})"),
    }

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
    // The detail that only matters one turn later: a turn that reasoned and
    // then called a tool is illegal on the next request without this.
    for block in &message.content {
        if let taurus_provider::ContentBlock::Thinking { signature, .. } = block {
            println!(
                "  thinking block: signature {}",
                if signature.is_some() {
                    "carried"
                } else {
                    "missing — this turn cannot be replayed"
                }
            );
        }
    }
    if !malformed.is_empty() {
        println!("  malformed: {malformed:?}");
    }
    Ok(())
}
