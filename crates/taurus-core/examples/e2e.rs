//! End-to-end run of the whole harness against a live Ollama model.
//!
//! `cargo run -p taurus-core --example e2e -- <model>`
//!
//! Creates a throwaway workspace, gives the agent a task that requires reading
//! and writing files, and reports what actually happened. This is the headless
//! equivalent of driving the desktop app by hand, and it exercises every layer
//! except the Tauri IPC bridge.

use std::sync::Arc;

use taurus_core::{Agent, AgentConfig, Session, UiEvent};
use taurus_provider::{Message, Provider};
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SYSTEM: &str = "\
You are Taurus, a coding agent. Use tools to find things out; never guess at \
file contents. Read a file before editing it. Be brief.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.6:27b".into());

    // A real workspace on disk, seeded with something worth reading.
    let dir = tempfile::tempdir()?;
    let workspace = dir.path().canonicalize()?;
    std::fs::write(
        workspace.join("README.md"),
        "# Widget Service\n\nA service for managing widgets.\nOwner: platform team.\n",
    )?;
    std::fs::write(
        workspace.join("CHANGELOG.md"),
        "# Changelog\n\n## 0.2.0\n- Added widget search\n\n## 0.1.0\n- First release\n",
    )?;

    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL));
    let caps = provider.capabilities(&model).await?;
    println!(
        "model {model} — native_tools={} ctx={}",
        caps.native_tools, caps.context_length
    );
    println!("workspace {}\n", workspace.display());

    // AllowAll stands in for the user clicking Allow; the permission engine is
    // still in the path, so a bug that skipped it would not be hidden here.
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));
    let cancel = CancellationToken::new();
    let agent = Agent::new(
        provider,
        ToolRegistry::with_builtins(),
        ToolContext::new(workspace.clone(), permissions, cancel),
        AgentConfig {
            system_prompt: format!("{SYSTEM}\n\nYou are working in `{}`.", workspace.display()),
            max_iterations: 12,
            ..Default::default()
        },
    );

    let (tx, mut rx) = mpsc::channel(256);
    let printer = tokio::spawn(async move {
        let mut tool_calls = 0usize;
        while let Some(event) = rx.recv().await {
            match event {
                UiEvent::TextDelta { text } => {
                    print!("{text}");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                UiEvent::ToolProgress { label, .. } => println!("      · {label}"),
                UiEvent::ToolCallStarted { name, preview, .. } => {
                    tool_calls += 1;
                    println!("\n  → {name}: {preview}");
                }
                UiEvent::ToolCallFinished { ok, output, .. } => {
                    let first = output.lines().next().unwrap_or("");
                    println!("  {} {first}", if ok { "✓" } else { "✕" });
                }
                UiEvent::Compacted { messages_removed } => {
                    println!("  [compacted {messages_removed} messages]");
                }
                UiEvent::Error { message } => println!("  [error] {message}"),
                UiEvent::TurnFinished { stop_reason, usage } => {
                    println!(
                        "\n\n--- {stop_reason:?}, {} in / {} out ---",
                        usage.input_tokens, usage.output_tokens
                    );
                }
                UiEvent::IterationStarted { .. } | UiEvent::ThinkingDelta { .. } => {}
            }
        }
        tool_calls
    });

    let mut session = Session::new(&model);
    let outcome = agent
        .run_turn(
            &mut session,
            Message::user(
                "Read every markdown file in this workspace, then write a file called \
                 SUMMARY.md containing one bullet per file describing what it covers.",
            ),
            tx,
        )
        .await;

    let tool_calls = printer.await?;

    match outcome {
        Ok(outcome) => println!(
            "iterations: {}, tool calls: {}",
            outcome.iterations, tool_calls
        ),
        Err(e) => println!("turn failed: {e}"),
    }

    // The point of the exercise: did the file actually get written?
    let summary = workspace.join("SUMMARY.md");
    if summary.is_file() {
        println!("\nSUMMARY.md was created:\n---");
        println!("{}", std::fs::read_to_string(&summary)?);
        println!("---");
    } else {
        println!("\nSUMMARY.md was NOT created");
    }
    Ok(())
}
