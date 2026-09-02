//! `taurus review` — reading a turn back to an agent that did not write it.
//!
//! Listing is the default here for the reason it is in `taurus rewind`: naming
//! no turn is a question, and a command that starts a model round trip because
//! somebody left an argument off is a command that surprises people. So a bare
//! `taurus review` prints what there is to review, and `--turn` runs one.
//!
//! What it prints is the reviewer's own words plus two facts that are not in
//! them: which model produced it, and which of the turn's files it did not see.
//! A review that covered four of six files and did not say so reads as a clean
//! bill of health for all six.
//!
//! See [`taurus_host::review`] for what the reviewer is given and, more to the
//! point, what it is deliberately not.

use std::process::ExitCode;

use taurus_host::{sessions, Checkpoint, Host};
use tokio_util::sync::CancellationToken;

pub async fn run(
    host: &Host,
    session: Option<&str>,
    turn: Option<u32>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<ExitCode, String> {
    let workspace = host.workspace().await;
    let store = host.checkpoints().await;

    let session_id = match session {
        Some(id) => id.to_string(),
        None => {
            sessions::latest(&workspace)
                .ok_or_else(|| {
                    format!(
                        "no saved sessions for {}. Start one with `taurus repl`.",
                        workspace.display()
                    )
                })?
                .id
        }
    };

    let turns = store.turns(&session_id)?;
    if turns.is_empty() {
        println!("Session {session_id} changed no files, so there is nothing to review.");
        return Ok(ExitCode::SUCCESS);
    }

    let Some(turn) = turn else {
        list(&turns, &session_id);
        return Ok(ExitCode::SUCCESS);
    };

    // The same two a turn is run on, resolved the same way — a review that
    // quietly used something else would be a bill nobody authorised. `--provider`
    // and `--model` override it here exactly as they do for `taurus run`, which
    // is what makes "review it with the big model" expressible without moving
    // the conversation onto one.
    let (provider_id, model) = host.resolve_model(provider, model).await?;
    let provider = host.provider(&provider_id).await?;

    // Said before the wait rather than after it. On a local model this takes
    // minutes, and a command that prints nothing for that long looks hung.
    eprintln!("Reviewing turn {turn} with {model}, from a context that did not write it…");

    let report = host
        .review_turn(
            provider,
            &model,
            &session_id,
            turn,
            CancellationToken::new(),
        )
        .await?;

    println!();
    println!("{}", report.text.trim());
    println!();
    println!(
        "— {} file{} reviewed by {}, without the conversation that produced {}.",
        report.files,
        if report.files == 1 { "" } else { "s" },
        report.model,
        if report.files == 1 { "it" } else { "them" }
    );
    if !report.omitted.is_empty() {
        // Last, so it is the thing still on screen. This is the sentence that
        // stops the review above being read as covering the whole turn.
        println!();
        println!(
            "  ! Not shown to the reviewer, so nothing above covers {}:",
            if report.omitted.len() == 1 {
                "it"
            } else {
                "them"
            }
        );
        for path in &report.omitted {
            println!("      {path}");
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// What there is to review, newest last so the last line is the likely answer.
fn list(turns: &[Checkpoint], session_id: &str) {
    println!("Session {session_id}:");
    for checkpoint in turns {
        println!(
            "  {:>3}  {} file{}  {}",
            checkpoint.turn,
            checkpoint.files.len(),
            if checkpoint.files.len() == 1 {
                " "
            } else {
                "s"
            },
            checkpoint.prompt
        );
    }
    println!();
    println!("Review one with: taurus review --turn <n>");
}
