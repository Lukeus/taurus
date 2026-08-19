//! `taurus agents` — inspecting the sub-agent roster without opening the app.

use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::{Host, ProblemSource};

#[derive(Subcommand)]
pub enum AgentsCommand {
    /// List the sub-agents this workspace can delegate to.
    List,
    /// Report agent files that failed to load or cannot fully run here.
    Check,
}

pub async fn run(host: &Host, command: AgentsCommand) -> Result<ExitCode, String> {
    match command {
        AgentsCommand::List => {
            let agents = host.agents().await;
            for agent in &agents {
                let shadowed = match agent.shadows {
                    Some(tier) => format!("  (shadows the {} one)", tier.label()),
                    None => String::new(),
                };
                // The tier says which of two files won; this says whose file
                // it is. An agent read out of another client's directory is
                // the one a reader is most likely to be surprised by, and the
                // one they cannot edit here without a copy being made.
                let borrowed = match agent.forks_on_edit && agent.path.is_some() {
                    true => "  (borrowed — editing it here writes a copy)",
                    false => "",
                };
                println!(
                    "{:<24} {:<10}{shadowed}{borrowed}",
                    agent.name,
                    agent.tier.label()
                );
                println!("    {}", agent.description);
                println!(
                    "    tools: {}",
                    match &agent.tools {
                        Some(tools) => tools.join(", "),
                        None => "inherits the parent's".to_string(),
                    }
                );
                if let Some(model) = &agent.model {
                    match &agent.provider {
                        Some(provider) => println!("    model: {model} on {provider}"),
                        None => println!("    model: {model}"),
                    }
                }
                if let Some(path) = &agent.path {
                    println!("    file: {path}");
                }
                if let Some(reason) = &agent.degraded {
                    println!("    degraded: {reason}");
                }
            }

            // Said even when the roster is full, because the built-ins are
            // always in it and a reader who wants to add one still needs to
            // know where the files go.
            println!();
            println!(
                "{} agents, costing {} characters of every request.",
                agents.len(),
                host.roster_cost().await
            );
            println!(
                "Add one as ~/.taurus/agents/<name>.md or <workspace>/.taurus/agents/<name>.md."
            );
            Ok(ExitCode::SUCCESS)
        }

        AgentsCommand::Check => {
            // Agents only. This command's exit code is meant to gate CI on the
            // agent files a repository ships, and failing that build because an
            // MCP server the developer runs locally would not start reports the
            // wrong thing about the wrong repository.
            let problems = host.problems_from(&[ProblemSource::Agents]).await;
            let degraded: Vec<_> = host
                .agents()
                .await
                .into_iter()
                .filter(|a| a.degraded.is_some())
                .collect();

            for problem in &problems {
                println!("{}", problem.message);
            }
            for agent in &degraded {
                println!(
                    "degraded: {} — {}",
                    agent.name,
                    agent.degraded.as_deref().unwrap_or_default()
                );
            }

            if problems.is_empty() && degraded.is_empty() {
                println!("All agents loaded and can run as written here.");
                return Ok(ExitCode::SUCCESS);
            }
            // A non-zero exit makes this usable as a CI check on the agents that
            // ship with a repository.
            Ok(ExitCode::FAILURE)
        }
    }
}
