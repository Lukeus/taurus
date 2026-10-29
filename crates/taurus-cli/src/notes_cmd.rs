//! `taurus memory` — what earlier conversations left for the next one.
//!
//! The desktop app keeps these in a drawer. They are the same file, read the
//! same way, so a note written in the app is one a scripted run inherits and
//! one this can take back out again — which is the whole of the promise that
//! both frontends share `~/.taurus`.

use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::Host;

#[derive(Subcommand)]
pub enum NotesCommand {
    /// List what has been written down for this workspace, newest first.
    List,
    /// Write a memory of your own. It's read into every later conversation in
    /// this workspace, the way the model's own are.
    Add {
        /// What to carry forward: a sentence or two, up to 2 KB.
        text: String,
    },
    /// Change one memory's text, by the id `list` prints.
    Edit {
        /// The id shown in the first column of `taurus memory list`.
        id: String,
        /// The memory's new text.
        text: String,
    },
    /// Remove one memory, by the id `list` prints.
    Forget {
        /// The id shown in the first column of `taurus memory list`.
        id: String,
    },
}

pub async fn run(host: &Host, command: NotesCommand) -> Result<ExitCode, String> {
    match command {
        NotesCommand::List => {
            let notes = host.notes().await;
            if notes.is_empty() {
                println!("Nothing has been written down for this workspace yet.");
                println!(
                    "The model writes these itself, with `remember`, when it learns something \
                     worth carrying to the next conversation. Write one yourself with \
                     `taurus memory add`."
                );
                return Ok(ExitCode::SUCCESS);
            }

            for note in notes {
                // The short id is enough to name one by hand and is what
                // `forget` accepts as a prefix.
                // A note you wrote says so: the model's are the ones you'd
                // want to check.
                let by = if note.session == taurus_host::memory::BY_YOU {
                    "  by you"
                } else {
                    ""
                };
                println!(
                    "{}  {}{by}",
                    &note.id[..8.min(note.id.len())],
                    when(note.at)
                );
                for line in note.text.lines() {
                    println!("    {line}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        NotesCommand::Add { text } => {
            host.add_note(&text).await?;
            println!("Written. The next conversation here starts out knowing it.");
            Ok(ExitCode::SUCCESS)
        }

        NotesCommand::Edit { id, text } => {
            let notes = host.notes().await;
            let Some(note) = one_note(&notes, &id) else {
                return Ok(ExitCode::FAILURE);
            };
            host.edit_note(&note.id.clone(), &text).await?;
            println!("Changed: {}", first_line(&text));
            Ok(ExitCode::SUCCESS)
        }

        NotesCommand::Forget { id } => {
            let notes = host.notes().await;
            let Some(note) = one_note(&notes, &id) else {
                return Ok(ExitCode::FAILURE);
            };
            let text = note.text.clone();
            host.forget_note(&note.id.clone()).await?;
            println!("Forgotten: {}", first_line(&text));
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// The one note an id prefix names, or `None` after saying why not.
///
/// Matched by prefix, because the full id is a uuid and nobody is going to type
/// one. An ambiguous prefix is refused rather than resolved arbitrarily —
/// changing or deleting the wrong note is not something this can offer to undo.
fn one_note<'a>(notes: &'a [taurus_host::Note], id: &str) -> Option<&'a taurus_host::Note> {
    let matched: Vec<_> = notes.iter().filter(|n| n.id.starts_with(id)).collect();
    match matched.as_slice() {
        [] => {
            println!("No memory here starts with {id}.");
            None
        }
        [note] => Some(note),
        several => {
            println!("{id} matches {} notes. Use more of the id:", several.len());
            for note in several {
                println!(
                    "  {}  {}",
                    &note.id[..12.min(note.id.len())],
                    first_line(&note.text)
                );
            }
            None
        }
    }
}

/// Unix seconds as something a person reads, without pulling in a date library
/// for one column. Days are what matters here — "yesterday" is the question
/// these answer.
fn when(at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ago = now.saturating_sub(at);

    match ago {
        0..=59 => "just now".into(),
        60..=3599 => plural(ago / 60, "minute"),
        3600..=86_399 => plural(ago / 3600, "hour"),
        _ => plural(ago / 86_400, "day"),
    }
}

fn plural(n: u64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{n} {unit}s ago")
    }
}

fn first_line(text: &str) -> String {
    const MAX: usize = 60;
    let line = text.lines().next().unwrap_or("").trim();
    match line.char_indices().nth(MAX) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recent_note_is_described_in_the_units_that_matter() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(when(now), "just now");
        assert_eq!(when(now - 120), "2 minutes ago");
        assert_eq!(when(now - 7200), "2 hours ago");
        assert_eq!(when(now - 86_400 * 3), "3 days ago");
    }

    #[test]
    fn one_of_anything_is_not_written_as_one_somethings() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(when(now - 60), "1 minute ago");
        assert_eq!(when(now - 3600), "1 hour ago");
        assert_eq!(when(now - 86_400), "1 day ago");
    }

    #[test]
    fn a_note_from_the_future_does_not_underflow() {
        // A clock that moved backwards, or a file copied from another machine.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(when(now + 3600), "just now");
    }

    #[test]
    fn a_long_note_is_shortened_for_a_one_line_report() {
        assert!(first_line(&"z".repeat(200)).ends_with('…'));
        assert_eq!(first_line("short\nand more"), "short");
    }
}
