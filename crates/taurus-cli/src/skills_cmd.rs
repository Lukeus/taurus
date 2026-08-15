//! `taurus skills` — inspecting the library without opening the app.

use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::{Host, ProblemSource};

#[derive(Subcommand)]
pub enum SkillsCommand {
    /// List discovered skills.
    List,
    /// Report skills that failed to load or cannot fully run here.
    Check,
}

pub async fn run(host: &Host, command: SkillsCommand) -> Result<ExitCode, String> {
    match command {
        SkillsCommand::List => {
            let skills = host.skills().await;
            if skills.is_empty() {
                println!("No skills found.");
                // Named in full, because "no skills found" is only actionable
                // if you know which directories were actually read — and there
                // are now six of them, not two.
                println!("Looked in, in order:");
                for source in host.skill_sources().await {
                    println!("  {}", source.dir.display());
                }
                return Ok(ExitCode::SUCCESS);
            }
            for skill in skills {
                let degraded = if skill.degraded.is_some() {
                    "  [scripts unavailable]"
                } else {
                    ""
                };
                println!(
                    "{:<24} {:<8} {:<7}{degraded}",
                    skill.name,
                    format!("{:?}", skill.tier).to_lowercase(),
                    format!("{:?}", skill.origin).to_lowercase()
                );
                println!("    {}", skill.when_to_use);
            }
            Ok(ExitCode::SUCCESS)
        }

        SkillsCommand::Check => {
            // Skills only. This command's exit code is meant to gate CI on a
            // skill library, and failing that build because an MCP server the
            // developer runs locally would not start reports the wrong thing
            // about the wrong repository.
            let problems = host.problems_from(&[ProblemSource::Skills]).await;
            let skills = host.skills().await;
            let degraded: Vec<_> = skills.iter().filter(|s| s.degraded.is_some()).collect();
            let warned: Vec<_> = skills.iter().filter(|s| !s.warnings.is_empty()).collect();

            for problem in &problems {
                println!("could not load: {}", problem.message);
            }
            for skill in &degraded {
                println!(
                    "degraded: {} — {}",
                    skill.name,
                    skill.degraded.as_deref().unwrap_or_default()
                );
            }
            // Reported but never fatal. These skills work; the exit code is a
            // CI gate, and failing a build over a skill that loaded fine —
            // often one written for another client and not ours to fix — would
            // make the gate something people switch off.
            for skill in &warned {
                for warning in &skill.warnings {
                    println!("warning: {} — {warning}", skill.name);
                }
            }

            if problems.is_empty() && degraded.is_empty() {
                if warned.is_empty() {
                    println!("All skills loaded and are fully runnable here.");
                } else {
                    println!(
                        "All skills loaded and are fully runnable here, with the warnings above."
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }
            // A non-zero exit makes this usable as a CI check on a skill
            // library that ships with a repository.
            Ok(ExitCode::FAILURE)
        }
    }
}
