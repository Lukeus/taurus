//! End-to-end skill authoring against a live Ollama model.
//!
//! `cargo run -p taurus-skills --example synthesis -- <model>`
//!
//! Runs the whole loop the desktop app runs: the agent works out a procedure,
//! calls `propose_skill`, the proposal is validated, "approved" on the user's
//! behalf, written to disk, and rediscovered by the catalog. If the skill it
//! wrote does not load back, that is a failure — a proposal that saves but
//! cannot be reloaded is worse than one that is rejected.

use std::sync::Arc;

use taurus_core::{Agent, AgentConfig, Session, UiEvent};
use taurus_provider::{Message, Provider};
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_skills::catalog::{SkillCatalog, SkillSource};
use taurus_skills::proposal::{save, validate_proposal, CollectingSink};
use taurus_skills::skill::SkillTier;
use taurus_skills::{LoadSkill, ProposeSkill, RunSkillScript};
use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.6:27b".into());

    let dir = tempfile::tempdir()?;
    let workspace = dir.path().canonicalize()?;
    let skills_root = workspace.join(".taurus/skills");
    std::fs::create_dir_all(&skills_root)?;

    let catalog = Arc::new(RwLock::new(SkillCatalog::default()));
    let sink = Arc::new(CollectingSink::default());

    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(LoadSkill::new(catalog.clone())));
    registry.register(Arc::new(RunSkillScript::new(catalog.clone())));
    registry.register(Arc::new(ProposeSkill::new(catalog.clone(), sink.clone())));

    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL));
    let caps = provider.capabilities(&model).await?;
    println!("model {model} — native_tools={}\n", caps.native_tools);

    let permissions = Arc::new(PermissionEngine::new(&workspace, Box::new(AllowAll)));
    let agent = Agent::new(
        provider,
        registry,
        ToolContext::new(workspace.clone(), permissions, CancellationToken::new()),
        AgentConfig {
            system_prompt: format!(
                "You are Taurus, a coding agent working in `{}`.\n\n\
                 When you work out a procedure that will come up again, call `propose_skill` to \
                 write it down. The user reviews every proposal, so proposing is cheap.",
                workspace.display()
            ),
            max_iterations: 8,
            ..Default::default()
        },
    );

    let (tx, mut rx) = mpsc::channel(256);
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                UiEvent::ToolCallStarted { name, preview, .. } => {
                    println!("  → {name}: {preview}");
                }
                UiEvent::ToolCallFinished { ok, output, .. } => {
                    println!(
                        "  {} {}",
                        if ok { "✓" } else { "✕" },
                        output.lines().next().unwrap_or("")
                    );
                }
                _ => {}
            }
        }
    });

    let mut session = Session::new(&model);
    let _ = agent
        .run_turn(
            &mut session,
            Message::user(
                "Here is how to cut a release in this project, which took me a while to work \
                 out: run `cargo test --workspace`, then bump the version in Cargo.toml, then tag \
                 with `git tag -a vX.Y.Z`, then push the tag. The tag must be annotated or the \
                 CI release job skips it. Write this down as a skill so you have it next time. \
                 Do not run any of the commands now.",
            ),
            tx,
        )
        .await;
    printer.await?;

    // Everything below is what the UI's approval card does.
    let proposals = sink.proposals.lock().await;
    if proposals.is_empty() {
        println!("\nNo skill was proposed. (Small models often skip this; try a larger one.)");
        return Ok(());
    }

    for proposal in proposals.iter() {
        println!("\n--- proposed: {} ---", proposal.name);
        println!("when_to_use: {}", proposal.when_to_use);
        println!("{}", proposal.body);

        match validate_proposal(proposal, &*catalog.read().await) {
            Ok(()) => {
                let dir = save(proposal, &skills_root)?;
                println!("saved to {}", dir.display());
            }
            Err(e) => {
                println!("rejected by validation: {e}");
                continue;
            }
        }
    }
    drop(proposals);

    // The real assertion: does it come back?
    let (reloaded, problems) = SkillCatalog::discover(&[SkillSource {
        tier: SkillTier::Project,
        dir: skills_root.clone(),
    }]);
    println!(
        "\nreloaded {} skill(s), {} problem(s)",
        reloaded.len(),
        problems.len()
    );
    for problem in &problems {
        println!("  problem: {problem}");
    }
    if let Some(section) = reloaded.prompt_section() {
        println!("\nwhat the next session's system prompt would carry:\n{section}");
    }
    Ok(())
}
