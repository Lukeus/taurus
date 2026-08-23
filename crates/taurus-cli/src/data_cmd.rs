//! `taurus data` — looking at what is loaded, and running a recipe.
//!
//! The half of the data feature that does not need a model. A recipe is a
//! committed file that turns one dataset into another, which makes it exactly
//! the kind of thing somebody wants in a `make` target or a CI step, and
//! needing a provider configured to run one would be a strange requirement for
//! a deterministic chain of SQL.

use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::Host;

#[derive(Subcommand)]
pub enum DataCommand {
    /// List the datasets loaded here and the recipes this workspace has.
    List,
    /// Run a recipe and write the file it names.
    Run {
        /// The recipe's name — its filename in .taurus/recipes without the .sql.
        name: String,
    },
}

pub async fn run(host: &Host, command: DataCommand) -> Result<ExitCode, String> {
    match command {
        DataCommand::List => {
            let datasets = host.datasets().await;
            if datasets.is_empty() {
                println!("No datasets loaded in this workspace.");
            } else {
                println!("Datasets");
                let width = datasets
                    .iter()
                    .map(|d| d.name.chars().count())
                    .max()
                    .unwrap_or(0);
                for dataset in &datasets {
                    println!(
                        "  {:width$}  {}  {}",
                        dataset.name,
                        dataset.path,
                        format!("{:?}", dataset.format).to_lowercase(),
                        width = width
                    );
                }
            }

            let (recipes, problems) = host.recipes().await;
            println!();
            if recipes.is_empty() {
                println!("No recipes. A recipe is a .sql file in {} with `source:` and `output:` in a --- frontmatter block, and its steps below.", taurus_data::RECIPE_DIR);
            } else {
                println!("Recipes");
                let width = recipes
                    .iter()
                    .map(|r| r.name.chars().count())
                    .max()
                    .unwrap_or(0);
                for recipe in &recipes {
                    println!(
                        "  {:width$}  {} → {}  ({} step{})",
                        recipe.name,
                        recipe.source,
                        recipe.output,
                        recipe.steps.len(),
                        if recipe.steps.len() == 1 { "" } else { "s" },
                        width = width
                    );
                    if let Some(description) = &recipe.description {
                        println!("  {:width$}  {description}", "", width = width);
                    }
                }
            }
            // Below the list rather than instead of it: a file somebody is
            // halfway through writing should not hide the four that work.
            for problem in &problems {
                println!("  ! {problem}");
            }
            Ok(ExitCode::SUCCESS)
        }

        DataCommand::Run { name } => {
            let (recipes, _) = host.recipes().await;
            let recipe = recipes.iter().find(|r| r.name == name);
            if let Some(recipe) = recipe {
                println!("{} → {}", recipe.source, recipe.output);
            }

            let run = match host.run_recipe(&name).await {
                Ok(run) => run,
                Err(error) => {
                    eprintln!("{error}");
                    return Ok(ExitCode::FAILURE);
                }
            };

            // The deltas are the point. A step meant to drop a hundred
            // duplicates that dropped four hundred thousand rows is invisible
            // in the SQL and unmissable in this column.
            let width = run
                .steps
                .iter()
                .map(|s| s.title.chars().count())
                .max()
                .unwrap_or(0);
            let mut previous = run.started_with;
            println!("{:>13} rows to start", thousands(run.started_with));
            for (index, step) in run.steps.iter().enumerate() {
                println!(
                    "{:>13} {:>10}  {}. {:width$}  {} ms",
                    thousands(step.rows),
                    delta(previous, step.rows),
                    index + 1,
                    step.title,
                    step.took_ms,
                    width = width
                );
                previous = step.rows;
            }
            println!(
                "\nWrote {} rows × {} columns.",
                thousands(run.rows),
                run.columns.len()
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// How a step changed the row count, or that it did not.
fn delta(before: u64, after: u64) -> String {
    match after.cmp(&before) {
        std::cmp::Ordering::Equal => "—".to_string(),
        std::cmp::Ordering::Less => format!("−{}", thousands(before - after)),
        std::cmp::Ordering::Greater => format!("+{}", thousands(after - before)),
    }
}

/// Digits grouped, so a seven-figure row count can be read at a glance.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_that_changed_nothing_says_so_rather_than_showing_a_zero() {
        assert_eq!(delta(100, 100), "—");
        assert_eq!(delta(400_000, 219_922), "−180,078");
        assert_eq!(delta(3, 9), "+6");
    }
}
