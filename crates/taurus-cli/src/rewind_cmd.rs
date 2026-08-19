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
    for outcome in &plan.restored {
        println!("  {}", describe(outcome));
    }
    // Last, so they are what is still on screen when the prompt below asks.
    // These are the reasons a rewind is not the whole way back, and burying
    // them above a list of forty files would be the same as not printing them.
    for warning in &plan.warnings {
        println!("\n  ! {}", wrapped(warning));
    }

    if dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    if !confirm(assume_yes, plan.restored.len())? {
        println!("\nLeft alone.");
        return Ok(ExitCode::SUCCESS);
    }

    let done = store.rewind(&session_id, &workspace, turn, false)?.restored;
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
        // Only when there is something to say. A conversation that stayed on
        // one branch and committed nothing is the common case, and a line of
        // empty fields under every turn would make the list harder to read
        // rather than more complete.
        let mut notes = Vec::new();
        if let Some(sha) = &turn.commit {
            notes.push(format!("committed as {sha}"));
        }
        if turn.moved_git {
            notes.push("moved git's own state".to_string());
        }
        if let Some(branch) = &turn.branch {
            notes.push(format!("on {branch}"));
        }
        if !notes.is_empty() {
            println!("  {:9} {}", "", notes.join(" · "));
        }
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

/// Folds a warning to a readable column, hanging under its marker.
///
/// The only thing this command wraps, and the only thing worth wrapping: every
/// other line it prints is a path or a short label the terminal can fold
/// wherever it likes. These are three sentences of prose hanging off a `!`, and
/// left to the terminal they arrive as a paragraph-shaped smear starting back
/// at column zero — which is how a warning gets skipped.
///
/// A fixed width rather than the terminal's. Asking would mean a dependency for
/// a number that is wrong the moment the output is piped, and 72 leaves room
/// for the marker inside the 80 columns that is still the narrow case.
fn wrapped(warning: &str) -> String {
    const WIDTH: usize = 72;
    const HANGING: &str = "\n    ";

    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in warning.split_whitespace() {
        // Counted in characters rather than bytes: a warning quoting a branch
        // name is prose, and prose is not always ASCII.
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > WIDTH {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines.join(HANGING)
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
                branch: None,
                moved_git: false,
                commit: None,
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
    fn a_warning_is_folded_under_its_own_marker() {
        let folded = wrapped(
            "Turn 4 moved git's own state. Its files come back; HEAD and the index \
             stay where the command left them, so the result will match neither \
             commit. `git reflog` is the way back to where HEAD was.",
        );
        let lines: Vec<&str> = folded.lines().collect();
        assert!(lines.len() > 1, "a long warning has to fold: {folded}");
        assert!(lines[0].chars().count() <= 72, "{:?}", lines[0]);
        assert!(
            lines[1..].iter().all(|l| l.starts_with("    ")),
            "continuations hang under the marker: {folded}"
        );
        // Folding is not editing: every word survives, in order.
        assert_eq!(
            folded.split_whitespace().collect::<Vec<_>>().join(" "),
            "Turn 4 moved git's own state. Its files come back; HEAD and the index \
             stay where the command left them, so the result will match neither \
             commit. `git reflog` is the way back to where HEAD was."
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn a_short_warning_is_left_on_one_line() {
        let folded = wrapped("Turn 2 was committed as a1b2c3d.");
        assert_eq!(folded, "Turn 2 was committed as a1b2c3d.");
    }

    #[test]
    fn a_single_word_longer_than_the_column_is_not_broken() {
        // A branch name or a path can be longer than the column on its own.
        // Splitting it would produce something that is not the name.
        let long = "a".repeat(90);
        assert_eq!(wrapped(&long), long);
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
