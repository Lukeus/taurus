//! `taurus rewind` — putting files back the way a turn found them.
//!
//! Listing is the default and restoring takes a flag, because the two differ in
//! kind: one prints, the other overwrites files in the workspace and cannot
//! itself be undone. The same reasoning makes a rewind ask before it writes,
//! and refuse rather than assume when there is no terminal to ask on — the
//! shape the permission gate already uses for a piped run.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use taurus_host::{sessions, Checkpoint, Host, Restored};

pub async fn run(
    host: &Host,
    session: Option<&str>,
    to: Option<&str>,
    dry_run: bool,
    assume_yes: bool,
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
        println!("Session {session_id} changed no files.");
        return Ok(ExitCode::SUCCESS);
    }

    let Some(to) = to else {
        list(&turns, &session_id);
        return Ok(ExitCode::SUCCESS);
    };

    let turn = resolve_turn(to, &turns)?;
    let plan = store.rewind(&session_id, &workspace, turn, true)?;

    println!(
        "Rewinding to before turn {turn} undoes {} turn{} in {}:\n",
        turns.len() - turn as usize + 1,
        if turns.len() - turn as usize + 1 == 1 {
            ""
        } else {
            "s"
        },
        workspace.display()
    );
    for outcome in &plan {
        println!("  {}", describe(outcome));
    }

    if dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    if !confirm(assume_yes, plan.len())? {
        println!("\nLeft alone.");
        return Ok(ExitCode::SUCCESS);
    }

    let done = store.rewind(&session_id, &workspace, turn, false)?;
    let count = |matches: fn(&Restored) -> bool| done.iter().filter(|r| matches(r)).count();
    let reverted = count(|r| matches!(r, Restored::Reverted { .. }));
    let deleted = count(|r| matches!(r, Restored::Deleted { .. }));
    let skipped = count(|r| matches!(r, Restored::Skipped { .. }));

    // Only what did not go to plan is reprinted; the plan above already said
    // what was meant to happen, and repeating it in full buries the exceptions.
    println!();
    for outcome in done
        .iter()
        .filter(|r| matches!(r, Restored::Skipped { .. }))
    {
        println!("  {}", describe(outcome));
    }
    println!("Reverted {reverted}, removed {deleted}, left alone {skipped}.");

    // Non-zero when the workspace is not actually back where it was asked to
    // be, so a script does not carry on believing it is.
    Ok(if skipped == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Newest first, which is the end of the list a person is usually reaching for.
fn list(turns: &[Checkpoint], session_id: &str) {
    println!("Checkpointed turns in session {session_id}, newest first:\n");
    for turn in turns.iter().rev() {
        let label = if turn.prompt.is_empty() {
            "(no prompt recorded)"
        } else {
            &turn.prompt
        };
        println!("  turn {:<4} {label}", turn.turn);
        println!("  {:9} {}", "", turn.files.join(", "));
    }
    println!(
        "\nUndo the last one with:  taurus rewind --to last\n\
         See what that would do:  taurus rewind --to last --dry-run"
    );
}

fn resolve_turn(raw: &str, turns: &[Checkpoint]) -> Result<u32, String> {
    if raw.eq_ignore_ascii_case("last") {
        return turns
            .last()
            .map(|t| t.turn)
            .ok_or_else(|| "there are no checkpointed turns".to_string());
    }
    raw.parse::<u32>().map_err(|_| {
        format!("'{raw}' is not a turn number. Use one from `taurus rewind`, or `last`.")
    })
}

fn describe(outcome: &Restored) -> String {
    match outcome {
        Restored::Reverted { path } => format!("reverted  {path}"),
        Restored::Deleted { path } => format!("deleted   {path}"),
        Restored::Skipped { path, reason } => format!("skipped   {path} — {reason}"),
    }
}

/// Asks before overwriting, or names the flag that would have permitted it.
///
/// A rewind discards whatever is in those files now, including edits the user
/// made by hand after the turn. Silently proceeding in a pipe would make that
/// invisible.
fn confirm(assume_yes: bool, files: usize) -> Result<bool, String> {
    if assume_yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "no terminal to confirm on; re-run with --yes to rewind, or --dry-run to see the plan"
                .into(),
        );
    }

    let mut err = std::io::stderr();
    let _ = write!(
        err,
        "\nOverwrite {files} file(s) with what was there before? [y/N]: "
    );
    let _ = err.flush();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turns() -> Vec<Checkpoint> {
        (1..=3)
            .map(|turn| Checkpoint {
                turn,
                prompt: format!("turn {turn}"),
                at: 0,
                files: vec!["a.txt".into()],
            })
            .collect()
    }

    #[test]
    fn last_resolves_to_the_most_recent_turn() {
        assert_eq!(resolve_turn("last", &turns()).unwrap(), 3);
        assert_eq!(resolve_turn("LAST", &turns()).unwrap(), 3);
    }

    #[test]
    fn a_turn_number_is_taken_as_written() {
        assert_eq!(resolve_turn("2", &turns()).unwrap(), 2);
    }

    #[test]
    fn anything_else_says_what_it_expected() {
        let err = resolve_turn("yesterday", &turns()).unwrap_err();
        assert!(err.contains("turn number"), "{err}");
        assert!(err.contains("last"), "{err}");
    }

    #[test]
    fn a_piped_rewind_names_the_flag_that_would_have_allowed_it() {
        // stdin is not a terminal under `cargo test`, which is the case this
        // guards: an unattended rewind must not just happen.
        let err = confirm(false, 3).unwrap_err();
        assert!(err.contains("--yes"), "{err}");
        assert!(err.contains("--dry-run"), "{err}");
    }

    #[test]
    fn yes_skips_the_prompt() {
        assert!(confirm(true, 3).unwrap());
    }

    #[test]
    fn every_outcome_names_its_file() {
        for outcome in [
            Restored::Reverted { path: "a".into() },
            Restored::Deleted { path: "a".into() },
            Restored::Skipped {
                path: "a".into(),
                reason: "because".into(),
            },
        ] {
            assert!(describe(&outcome).contains('a'));
        }
        assert!(describe(&Restored::Skipped {
            path: "a".into(),
            reason: "because".into(),
        })
        .contains("because"));
    }
}
