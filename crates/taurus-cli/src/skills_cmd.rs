//! `taurus skills` — inspecting the library without opening the app.

use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::Host;

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
                println!("Looked in ~/.taurus/skills and <workspace>/.taurus/skills.");
                return Ok(ExitCode::SUCCESS);
            }
            for skill in skills {
                let degraded = if skill.degraded.is_some() {
                    "  [scripts unavailable]"
                } else {
                    ""
                };
                println!("{:<24} {:<8}{degraded}", skill.name, format!("{:?}", skill.tier).to_lowercase());
                println!("    {}", skill.when_to_use);
            }
            Ok(ExitCode::SUCCESS)
        }

        SkillsCommand::Check => {
            let problems = host.problems().await;
            let degraded: Vec<_> = host
                .skills()
                .await
                .into_iter()
                .filter(|s| s.degraded.is_some())
                .collect();

            for problem in &problems {
                println!("could not load: {problem}");
            }
            for skill in &degraded {
                println!(
                    "degraded: {} — {}",
                    skill.name,
                    skill.degraded.as_deref().unwrap_or_default()
                );
            }

            if problems.is_empty() && degraded.is_empty() {
                println!("All skills loaded and are fully runnable here.");
                return Ok(ExitCode::SUCCESS);
            }
            // A non-zero exit makes this usable as a CI check on a skill
            // library that ships with a repository.
            Ok(ExitCode::FAILURE)
        }
    }
}
