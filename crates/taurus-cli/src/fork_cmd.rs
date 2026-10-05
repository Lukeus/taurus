//! `taurus fork` — trying a turn again, keeping the first attempt.
//!
//! Listing is the default, the way `taurus rewind` lists. `--at N` starts a
//! new conversation from just before turn N and puts the files back to that
//! point; `--switch` puts a conversation's files back after another branch of
//! it took the workspace; `--compare` sets two branches side by side without
//! switching. Forking and switching don't ask first, unlike a rewind, because
//! neither loses anything: whatever is on disk is set aside before it's
//! written over, and a file that can't be set aside is left alone. See
//! `taurus_host::fork`.

use std::process::ExitCode;

use taurus_host::{fork, review, sessions, Comparison, Host, Restored, TurnChange};

use crate::rewind_cmd::{describe, resolve_turn, wrapped};

/// What `taurus fork` was asked to do.
pub enum Action<'a> {
    List,
    At {
        turn: &'a str,
        resend: bool,
        /// What to ask instead of the forked turn's question.
        ask: Option<&'a str>,
    },
    Switch,
    Compare {
        other: &'a str,
    },
}

/// Forks, switches or lists, and says what the next step is.
///
/// Returns the new conversation and the prompt to ask when `--resend` or
/// `--ask` asked for one, so the caller can run that turn with the provider
/// and model it was given.
pub async fn run(
    host: &Host,
    id: Option<&str>,
    action: Action<'_>,
    dry_run: bool,
) -> Result<(ExitCode, Option<(String, String)>), String> {
    let workspace = host.workspace().await;
    let store = host.checkpoints().await;

    let session_id = match id {
        Some(id) => id.to_string(),
        None if matches!(action, Action::Switch) => {
            return Err(
                "name the conversation to switch to: taurus fork --switch --id <ID>. \
                 `taurus sessions` lists them, and marks the ones whose files are set aside."
                    .into(),
            )
        }
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

    match action {
        Action::List => {
            let turns = store.turns(&session_id)?;
            if let Some(other) = store.away(&session_id)? {
                println!(
                    "Session {session_id}'s files were set aside when {other} took the \
                     workspace.\nPut them back with:  taurus fork --switch --id {session_id}\n"
                );
            }
            if turns.is_empty() {
                println!(
                    "Session {session_id} changed no files, so there's no turn to fork before."
                );
                return Ok((ExitCode::SUCCESS, None));
            }
            println!("Turns in session {session_id} you can fork before, newest first:\n");
            for turn in turns.iter().rev() {
                let label = if turn.prompt.is_empty() {
                    "(no prompt recorded)"
                } else {
                    &turn.prompt
                };
                println!("  turn {:<4} {label}", turn.turn);
            }
            println!(
                "\nTry the last one again:   taurus fork --at last\n\
                 Ask it again right away:  taurus fork --at last --resend\n\
                 Ask it differently:       taurus fork --at last --ask \"…\"\n\
                 See what it would change: taurus fork --at last --dry-run"
            );
            Ok((ExitCode::SUCCESS, None))
        }

        Action::At { turn, resend, ask } => {
            let turns = store.turns(&session_id)?;
            let turn = resolve_turn(turn, &turns)?;
            let forked = {
                let (store, workspace, session_id) =
                    (store.clone(), workspace.clone(), session_id.clone());
                tokio::task::spawn_blocking(move || {
                    fork::fork(&store, &workspace, &session_id, turn, dry_run)
                })
                .await
                .map_err(|e| e.to_string())??
            };

            let verb = if dry_run {
                "Forking would put"
            } else {
                "Forked. Put"
            };
            // The turn the files went back to, which isn't always the one
            // asked for: inside a continued turn, the fork goes back to where
            // its request began.
            println!(
                "{verb} {} back the way it was before turn {}:\n",
                workspace.display(),
                forked.turn
            );
            print_outcomes(&forked.restored);
            for warning in &forked.warnings {
                println!("\n  ! {}", wrapped(warning));
            }
            let Some(new_id) = forked.id else {
                return Ok((ExitCode::SUCCESS, None));
            };
            println!(
                "\nThe new conversation is {new_id}. Session {session_id} is kept as it \
                 was, with its files set aside.\n\
                 Continue the fork:       taurus repl --resume {new_id}\n\
                 Go back to the original: taurus fork --switch --id {session_id}"
            );
            let code = exit_code(&forked.restored);
            let asking = match ask {
                Some(text) => Some(text.to_string()),
                None => resend.then_some(forked.prompt),
            };
            Ok((code, asking.map(|prompt| (new_id, prompt))))
        }

        Action::Compare { other } => {
            let compared = {
                let (store, workspace, a, b) = (
                    store.clone(),
                    workspace.clone(),
                    session_id.clone(),
                    other.to_string(),
                );
                tokio::task::spawn_blocking(move || fork::compare(&store, &workspace, &a, &b))
                    .await
                    .map_err(|e| e.to_string())??
            };
            print_comparison(&compared);
            Ok((ExitCode::SUCCESS, None))
        }

        Action::Switch => {
            let switched = {
                let (store, workspace, session_id) =
                    (store.clone(), workspace.clone(), session_id.clone());
                tokio::task::spawn_blocking(move || {
                    fork::switch(&store, &workspace, &session_id, dry_run)
                })
                .await
                .map_err(|e| e.to_string())??
            };
            let verb = if dry_run {
                "Switching would put"
            } else {
                "Put"
            };
            println!(
                "{verb} session {session_id}'s files back in {}:\n",
                workspace.display()
            );
            print_outcomes(&switched.restored);
            if let (Some(from), false) = (&switched.from, dry_run) {
                println!(
                    "\nSession {from}'s files are set aside. Back again with:  \
                     taurus fork --switch --id {from}"
                );
            }
            Ok((exit_code(&switched.restored), None))
        }
    }
}

fn print_comparison(compared: &Comparison) {
    println!(
        "Two branches of one conversation. They share {}.\n",
        plural(compared.shared as usize, "question")
    );
    for (mark, side) in [("A", &compared.a), ("B", &compared.b)] {
        let place = if side.on_disk {
            "has the workspace"
        } else {
            "set aside"
        };
        println!("  {mark}  {}  ({place})", side.id);
        if side.asked.is_empty() {
            println!("     Asked nothing since they parted.");
        }
        for asked in &side.asked {
            println!("     asked   {}", first_line(asked));
        }
        for turn in &side.turns {
            let reviewed = side
                .reviews
                .iter()
                .find(|review| review.turn == turn.turn)
                .map(|review| format!(", reviewed by {}", review.model))
                .unwrap_or_default();
            println!(
                "     turn {:<3} {}{reviewed}",
                turn.turn,
                plural(turn.files.len(), "file")
            );
        }
        println!(
            "     spent   {} in / {} out\n",
            side.usage.input_tokens, side.usage.output_tokens
        );
    }

    if compared.files.is_empty() {
        println!("Their files are the same.");
        return;
    }
    let n = compared.files.len();
    println!(
        "{} {}, from A to B:\n",
        plural(n, "file"),
        if n == 1 { "differs" } else { "differ" }
    );
    for change in &compared.files {
        match change {
            TurnChange::Diff { diff } => print!("{}", review::unified(diff)),
            TurnChange::Opaque { path, reason } => println!("--- {path}: {reason}\n"),
        }
    }
    for side in [&compared.a, &compared.b] {
        if !side.on_disk {
            println!(
                "Switch to {}:  taurus fork --switch --id {}",
                side.id, side.id
            );
        }
    }
}

/// "1 file", "3 files".
fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// A question's first line, for a listing.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn print_outcomes(restored: &[Restored]) {
    if restored.is_empty() {
        println!("  (no files to change)");
    }
    for outcome in restored {
        println!("  {}", describe(outcome));
    }
}

/// Non-zero when a file was left alone, so a script doesn't carry on believing
/// the workspace is where it asked for.
fn exit_code(restored: &[Restored]) -> ExitCode {
    if restored
        .iter()
        .any(|r| matches!(r, Restored::Skipped { .. }))
    {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
