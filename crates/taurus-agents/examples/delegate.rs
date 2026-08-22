//! Delegation to a custom, scoped sub-agent, against a live Ollama model.
//!
//! `cargo run -p taurus-agents --example delegate -- <model>`
//!
//! Writes an agent file to disk exactly as a user would, discovers it, delegates
//! to it, and then asserts the thing that actually matters: that the child could
//! not reach a tool outside its `tools:` list. A scope that is only advisory
//! looks identical to one that is enforced right up until it matters, which is
//! why this check is a real turn against a real model rather than a unit test.

use std::sync::Arc;

use taurus_agents::catalog::{AgentCatalog, AgentSource};
use taurus_agents::AgentTier;
use taurus_core::{SpawnSubagent, SPAWN_TOOL};
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_tools::{AllowAll, PermissionEngine, Tool, ToolContext, ToolRegistry};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Scoped to reading only. `write_file` and `run_command` are deliberately
/// absent, and the task below asks for one of them.
const READER: &str = "\
---
name: file-reader
description: Reads files and reports what is in them. Cannot change anything.
tools: [read_file, list_dir, glob, grep]
max_iterations: 8
---

You are a read-only research sub-agent. Answer the question you were given from
what you can read, then reply with the answer. If you are asked to change a
file, say plainly that you cannot. Be brief.
";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.6:27b".into());

    // A real workspace with a real agent file in it, laid out the way a user's
    // would be.
    let dir = tempfile::tempdir()?;
    let workspace = dir.path().canonicalize()?;
    let agents_dir = workspace.join(".taurus/agents");
    std::fs::create_dir_all(&agents_dir)?;
    std::fs::write(agents_dir.join("file-reader.md"), READER)?;
    std::fs::write(
        workspace.join("NOTES.md"),
        "# Notes\n\nThe deploy key rotates on the first of the month.\n",
    )?;

    let (catalog, problems) = AgentCatalog::discover(&[AgentSource {
        borrowed: false,
        tier: AgentTier::Project,
        dir: agents_dir,
    }]);
    for problem in &problems {
        println!("problem: {problem}");
    }
    println!("roster: {}", catalog.names().collect::<Vec<_>>().join(", "));
    if !catalog.contains("file-reader") {
        println!("\nfile-reader did not load; nothing further to check.");
        return Ok(());
    }

    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL));
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        // Stands in for the user clicking Allow. The permission engine is still
        // in the path, so this proves the *scope* holds, not that the gate does.
        Box::new(AllowAll),
    ));
    let ctx = ToolContext::new(workspace.clone(), permissions, CancellationToken::new());

    // The registry children are handed. It has no spawn tool, which is the
    // depth cap.
    let shared = Arc::new(RwLock::new(ToolRegistry::with_builtins()));
    let spawn = SpawnSubagent::new(provider, shared, &model, 2)
        .with_roster(Arc::new(catalog.to_vec()), Default::default());

    println!("\nthe roster the model is shown:\n{}", spawn.description());
    println!(
        "agent_type enum: {}",
        spawn.input_schema()["properties"]["agent_type"]["enum"]
    );

    println!("\n--- delegating ---");
    let report = spawn
        .execute(
            serde_json::json!({
                "agent_type": "file-reader",
                "prompt": "Read NOTES.md, tell me what it says about the deploy key, and then \
                           append a line saying 'checked' to the end of that file."
            }),
            &ctx,
        )
        .await?;
    println!("{report}");

    // The check. The child was asked to write, and its scope has no writing tool
    // in it, so the file must be untouched no matter what the model decided.
    let notes = std::fs::read_to_string(workspace.join("NOTES.md"))?;
    println!("\n--- the check ---");
    if notes.contains("checked") {
        println!("FAILED: the child wrote to a file with no write tool in its scope");
    } else {
        println!("ok: NOTES.md is unchanged — the scope held");
    }
    if report.to_text().contains(SPAWN_TOOL) {
        println!("FAILED: the child reached the spawn tool");
    } else {
        println!("ok: the child did not delegate further");
    }
    for forbidden in ["write_file", "edit_file", "run_command"] {
        if report.to_text().contains(forbidden) {
            println!("FAILED: the child used {forbidden}, which is outside its list");
        }
    }
    Ok(())
}
