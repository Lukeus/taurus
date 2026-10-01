//! Live smoke test against any OpenAI-compatible endpoint.
//!
//! `cargo run -p taurus-provider-openai --example openai-smoke -- <model> [base_url] [--responses] [--effort <effort>]`
//!
//! Ollama serves an OpenAI-compatible API at /v1, so this runs against the
//! same local server as the Ollama adapter — a direct comparison of the two
//! code paths over identical hardware.
//!
//! `--responses` sends chat to the Responses API instead. Two requests either
//! way: the turn that calls the tool, then the one that reads its result. The
//! second is the half a mock can't vouch for, because on the Responses route
//! it carries the first turn's sealed reasoning back, and only the real API
//! can say whether it takes it.

use taurus_provider::{
    ChatRequest, ContentBlock, Message, Provider, Role, StopReason, StreamAccumulator, ToolDef,
};
use taurus_provider_openai::{OpenAiApi, OpenAiCapabilities, OpenAiProvider};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut api = OpenAiApi::ChatCompletions;
    let mut effort = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--responses" => api = OpenAiApi::Responses,
            "--effort" => effort = args.next(),
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    let model = positional
        .next()
        .unwrap_or_else(|| "llama3.2:latest".into());
    let base_url = positional
        .next()
        .unwrap_or_else(|| "http://localhost:11434".into());

    let provider = OpenAiProvider::new(
        "openai-compat",
        base_url,
        std::env::var("OPENAI_API_KEY").ok(),
        OpenAiCapabilities::default(),
    )
    .with_api(api)
    .with_reasoning_effort(effort.as_deref());

    let tools = vec![ToolDef {
        name: "list_dir".into(),
        description: "List the entries of a directory".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    }];
    let mut history = vec![Message::user(
        "List the contents of /etc. Call the tool, do not guess.",
    )];

    println!("model: {model} ({api:?})");
    for round in 1..=2 {
        let request = ChatRequest::new(&model, history.clone())
            .with_system("You are a terse assistant with tools.")
            .with_tools(tools.clone());
        println!("round {round}:");
        let (message, stop) = turn(&provider, request).await?;
        println!("  stop_reason: {stop:?}");
        let reasoned = message.content.iter().find_map(|b| match b {
            ContentBlock::Thinking { text, signature } => Some((text.len(), signature.is_some())),
            _ => None,
        });
        if let Some((chars, sealed)) = reasoned {
            println!("  reasoning: {chars} chars of summary, carried forward: {sealed}");
        }
        if !message.text().trim().is_empty() {
            println!("  text: {}", message.text().trim());
        }
        let calls: Vec<String> = message
            .tool_uses()
            .map(|(id, name, input)| {
                println!("  tool call: {name}({input}) [{id}]");
                id.to_string()
            })
            .collect();

        if stop != StopReason::ToolUse || calls.is_empty() {
            break;
        }
        history.push(message);
        history.push(Message::new(
            Role::User,
            calls
                .iter()
                .map(|id| ContentBlock::tool_result(id, "hosts\npasswd\nresolv.conf\nssh/"))
                .collect(),
        ));
    }
    Ok(())
}

async fn turn(
    provider: &OpenAiProvider,
    request: ChatRequest,
) -> Result<(Message, StopReason), Box<dyn std::error::Error>> {
    let (tx, mut rx) = mpsc::channel(64);
    let collect = tokio::spawn(async move {
        let mut acc = StreamAccumulator::new();
        while let Some(event) = rx.recv().await {
            acc.push(event);
        }
        acc.finish()
    });
    let stop = provider
        .stream(request, tx, CancellationToken::new())
        .await?;
    let (message, usage, malformed) = collect.await?;
    println!(
        "  usage: {} in / {} out{}",
        usage.input_tokens,
        usage.output_tokens,
        usage
            .reasoning_tokens
            .map(|r| format!(" ({r} reasoning)"))
            .unwrap_or_default()
    );
    if !malformed.is_empty() {
        println!("  malformed: {malformed:?}");
    }
    Ok((message, stop))
}
