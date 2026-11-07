//! `taurus delegates`: work delegates kept in their own copy of a workspace.
//!
//! Listing is the default, and applying asks first, for `rewind`'s reason: it
//! writes into the workspace. What it writes is one turn in the conversation's
//! undo history, so `taurus rewind` takes it back out.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use taurus_host::Host;
use taurus_tools::ApplyAction;

pub async fn run(
    host: &Host,
    apply: Option<&str>,
    discard: Option<&str>,
    dry_run: bool,
    assume_yes: bool,
) -> Result<ExitCode, String> {
    let runs = host.delegate_runs();
    let find = |run: &str| {
        runs.iter().find(|r| r.run == run).cloned().ok_or_else(|| {
            format!("no delegate run '{run}' is waiting. `taurus delegates` lists them.")
        })
    };

    if let Some(run) = discard {
        let found = find(run)?;
        let session = found
            .session
            .ok_or("that run has no conversation to discard it from")?;
        host.discard_delegate(&session, run).await?;
        println!("Discarded run {run}. Nothing from it reached the workspace.");
        return Ok(ExitCode::SUCCESS);
    }

    let Some(run) = apply else {
        if runs.is_empty() {
            println!("No delegate has work waiting.");
            return Ok(ExitCode::SUCCESS);
        }
        for r in &runs {
            println!(
                "{}  {} in {} ({}){}",
                r.run,
                r.agent,
                r.workspace,
                r.state,
                r.left_behind
                    .as_deref()
                    .map(|why| format!("\n    {why}"))
                    .unwrap_or_default()
            );
        }
        println!("\nApply one with `taurus delegates --apply <run>`, or drop it with --discard.");
        return Ok(ExitCode::SUCCESS);
    };

    let found = find(run)?;
    let session = found
        .session
        .ok_or("that run has no conversation to apply it in")?;
    let plan = host.apply_delegate(&session, run, true).await?;
    for file in &plan.files {
        let word = match file.action {
            ApplyAction::Take => "take",
            ApplyAction::Merge => "merge",
            ApplyAction::Delete => "delete",
            ApplyAction::Same => "same",
            ApplyAction::Conflict => "CAN'T",
        };
        match &file.why {
            Some(why) => println!("  {word:<6} {}: {why}", file.path),
            None => println!("  {word:<6} {}", file.path),
        }
    }
    if plan.conflicts().next().is_some() {
        println!("\n{}", plan.describe());
        return Ok(ExitCode::FAILURE);
    }
    if dry_run {
        println!("\n{}", plan.describe());
        return Ok(ExitCode::SUCCESS);
    }
    if !assume_yes {
        if !std::io::stdin().is_terminal() {
            return Err(
                "applying writes into the workspace; pass --yes to do it without a terminal".into(),
            );
        }
        print!("\nApply these to {}? [y/N] ", found.workspace);
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("Nothing applied.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let plan = host.apply_delegate(&session, run, false).await?;
    println!("{}", plan.describe());
    Ok(if plan.applied {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
