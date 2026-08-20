//! `taurus hooks` — seeing what will run, and why one is not running.
//!
//! A hook is invisible when it works: the whole point is that a turn behaves
//! slightly differently and nothing announces it. That makes the two failures
//! here hard to tell apart from a distance — a hook that is not configured, and
//! a hook that is configured wrong — so both are named rather than left to be
//! inferred from a turn that did not do what was expected.

use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::{Host, ProblemSource};

#[derive(Subcommand)]
pub enum HooksCommand {
    /// List the hooks that will run.
    List,
    /// Report entries that would not load.
    Check,
}

pub async fn run(host: &Host, command: HooksCommand) -> Result<ExitCode, String> {
    let problems = host.problems_from(&[ProblemSource::Hooks]).await;

    match command {
        HooksCommand::List => {
            let hooks = host.hook_summaries().await;
            if hooks.is_empty() {
                println!("No hooks configured.");
                // Named in full: "no hooks" is only actionable if you know
                // which files were read — and in an untrusted workspace one of
                // them deliberately was not.
                println!("Looked in:");
                for path in host.hook_files().await {
                    println!("  {}", path.display());
                }
            } else {
                for hook in hooks {
                    println!("{:<20} {:<18} {}", hook.name, hook.on.label(), hook.command);
                    if let Some(scope) = hook.matches {
                        println!("    on: {scope}");
                    }
                }
            }

            // Always, not only when the list is empty. A hooks file with one
            // working entry and one broken one is the case where a silent
            // listing is actively misleading.
            if !problems.is_empty() {
                println!();
                for problem in &problems {
                    println!("  {}", problem.message);
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        HooksCommand::Check => {
            if problems.is_empty() {
                println!("No problems.");
                return Ok(ExitCode::SUCCESS);
            }
            for problem in &problems {
                println!("{}", problem.message);
            }
            // A non-zero exit so this is usable in CI, the same as the other
            // `check` subcommands here.
            Ok(ExitCode::FAILURE)
        }
    }
}
