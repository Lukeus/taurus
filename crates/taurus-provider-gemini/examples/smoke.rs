//! Live smoke test against the Gemini generateContent API.
//!
//! `GEMINI_API_KEY=… cargo run -p taurus-provider-gemini --example smoke -- <model>`
//!
//! Exercises the parts unit tests cannot: that a real model's function call
//! reassembles through an adapter that has to synthesize the id the wire format
//! omits, and that the schema sanitizer produces a declaration this API accepts
//! rather than rejects.

use taurus_provider::{ChatRequest, Message, Provider, StreamAccumulator, ToolDef};
use taurus_provider_gemini::{GeminiProvider, DEFAULT_BASE_URL};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().unwrap_or_else(|| "gemini-2.5-pro".into());
    let base_url = args.next().unwrap_or_else(|| DEFAULT_BASE_URL.into());

    let key = std::env::var("GEMINI_API_KEY").ok();
    if key.is_none() {
        eprintln!("warning: GEMINI_API_KEY is unset; expect a credentials error");
    }
    let provider = GeminiProvider::new("gemini", base_url, key);

    // The window comes from the listing; tool and image support do not, so
    // those two stay configuration.
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
    // Ids here are this harness's own — the wire format carries none, so two
    // calls to the same tool would otherwise be indistinguishable.
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
