//! Comments a person leaves on the work, sent with their next message.
//!
//! A turn's diff and a plan's steps are things somebody reads and has an
//! opinion about at a particular line: "keep the old constructor too" means
//! nothing without the constructor it is about. So a comment is pinned to a
//! place, collected while the person reads, and goes out with whatever they
//! send next — never into a turn that is already running.
//!
//! # Into the transcript, unlike what was on screen
//!
//! [`crate::onscreen`] puts its context onto the prompt and keeps it off the
//! transcript, because a pane being open is a circumstance of the request and
//! not part of it. A comment is the opposite case: it is the person's own
//! words, and a conversation reopened next week that showed "fix these" with
//! nothing under it would have lost the half that mattered. So [`compose`]
//! produces the message itself — the one the transcript keeps, the model
//! reads, a retry resends, and a fork quotes — and there is one way to read it
//! back, which is as text.
//!
//! # Anchored by what the line says
//!
//! A line number is where a line was in the file the turn left behind, and the
//! next turn may have moved it. The line's own text is what survives that, so
//! it is quoted beside the number and the model can find the line by reading.
//! A plan step is anchored by its text for a different reason: steps have no
//! ids at all. See [`taurus_tools::plan`].

use serde::{Deserialize, Serialize};
use taurus_tools::DiffLineKind;
use ts_rs::TS;

/// At most this many comments go with one message.
///
/// Far past what anybody leaves on one read of a diff. It exists because the
/// list comes from a window over IPC, and a bound is cheaper to state than to
/// discover.
pub const MAX_REMARKS: usize = 50;

/// One comment's text, at most this many characters.
const MAX_TEXT: usize = 2_000;

/// The quoted line, at most this many characters.
///
/// A quote is there to be recognized, not read in full: the start of a line is
/// what tells two similar ones apart, and a minified line can run to megabytes.
const MAX_QUOTE: usize = 160;

/// A comment, and the place it is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Remark {
    pub anchor: Anchor,
    /// What the person wrote, as they wrote it.
    pub text: String,
}

/// Where a comment is pinned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Anchor {
    /// A line of a turn's diff.
    Diff {
        /// The checkpoint the diff was read from. Not said to the model, which
        /// has never seen a checkpoint number; kept so a frontend can tell
        /// which comments a rewind has made meaningless.
        turn: u32,
        /// Workspace-relative, as the diff names it.
        path: String,
        /// Which side of the diff the line is on.
        side: DiffLineKind,
        /// 1-based: in the file as the turn left it, or for a removed line, as
        /// it was before.
        line: u32,
        /// The line's text, which is what actually finds it again.
        quoted: String,
    },
    /// A step of the plan, by its text.
    Step { text: String },
}

/// The message to send: what was typed, followed by the comments.
///
/// Comments with nothing written in them are dropped rather than sent as a
/// bare location, which would read as a demand to change something without
/// saying what. With no comments left this is `text`, untouched.
///
/// Refuses a list over [`MAX_REMARKS`] instead of cutting it: silently sending
/// the first fifty of sixty comments is the one outcome worse than an error.
pub fn compose(text: &str, remarks: &[Remark]) -> Result<String, String> {
    if remarks.len() > MAX_REMARKS {
        return Err(format!(
            "{} comments is more than one message can carry ({MAX_REMARKS}). Send some of them first.",
            remarks.len()
        ));
    }
    let items: Vec<String> = remarks.iter().filter_map(item).collect();
    if items.is_empty() {
        return Ok(text.to_string());
    }
    let typed = text.trim();
    // A message that is only comments still has to say what they are for. A
    // small model handed a bare list will sometimes summarize it back.
    let head = if typed.is_empty() {
        "Please address these comments:"
    } else {
        "Comments:"
    };
    let mut out = String::new();
    if !typed.is_empty() {
        out.push_str(typed);
        out.push_str("\n\n");
    }
    out.push_str(head);
    out.push('\n');
    for item in items {
        out.push('\n');
        out.push_str(&item);
    }
    Ok(out)
}

/// One comment as a list item, or `None` when it says nothing.
fn item(remark: &Remark) -> Option<String> {
    let said = cut(remark.text.trim(), MAX_TEXT);
    if said.is_empty() {
        return None;
    }
    let place = match &remark.anchor {
        Anchor::Diff {
            path,
            side,
            line,
            quoted,
            ..
        } => {
            let quote = code(&cut(quoted.trim(), MAX_QUOTE));
            match side {
                DiffLineKind::Removed => {
                    format!("{} — removed line, was {line}: {quote}", code(path))
                }
                _ => format!("{} line {line}: {quote}", code(path)),
            }
        }
        Anchor::Step { text } => format!("Plan step \"{}\"", cut(text.trim(), MAX_QUOTE)),
    };
    // Continuation lines indented under the bullet, so a comment with a
    // paragraph break in it stays one list item in any Markdown reader.
    let body = said.lines().collect::<Vec<_>>().join("\n  ");
    Some(format!("- {place}\n  {body}"))
}

/// `text` as inline code, whatever backticks it holds.
///
/// The fence is one backtick longer than the longest run inside, and padded
/// when the text starts or ends with one — CommonMark's own rule, so a line of
/// Markdown or a shell command with backticks in it quotes cleanly.
fn code(text: &str) -> String {
    if text.is_empty() {
        return "(blank line)".into();
    }
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// At most `max` characters, marked when anything was cut. Characters rather
/// than bytes, so a cut never lands inside one.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_line(side: DiffLineKind, line: u32, quoted: &str, text: &str) -> Remark {
        Remark {
            anchor: Anchor::Diff {
                turn: 3,
                path: "src/widget.rs".into(),
                side,
                line,
                quoted: quoted.into(),
            },
            text: text.into(),
        }
    }

    fn on_step(step: &str, text: &str) -> Remark {
        Remark {
            anchor: Anchor::Step { text: step.into() },
            text: text.into(),
        }
    }

    #[test]
    fn no_comments_leaves_the_message_alone() {
        assert_eq!(compose("  hello  ", &[]).unwrap(), "  hello  ");
    }

    #[test]
    fn comments_follow_the_message() {
        let out = compose(
            "looks close",
            &[on_line(
                DiffLineKind::Added,
                42,
                "    pub fn new(name: &str) -> Self {",
                "keep the old constructor too",
            )],
        )
        .unwrap();
        assert_eq!(
            out,
            "looks close\n\nComments:\n\n\
             - `src/widget.rs` line 42: `pub fn new(name: &str) -> Self {`\n  \
             keep the old constructor too"
        );
    }

    #[test]
    fn a_message_of_only_comments_says_what_they_are_for() {
        let out = compose(
            "",
            &[on_step(
                "Add version to config.toml",
                "read it from Cargo.toml",
            )],
        )
        .unwrap();
        assert!(out.starts_with("Please address these comments:\n"), "{out}");
        assert!(
            out.contains("- Plan step \"Add version to config.toml\"\n  read it from Cargo.toml")
        );
    }

    #[test]
    fn the_turn_number_is_not_said_to_the_model() {
        let out = compose("", &[on_line(DiffLineKind::Added, 7, "x", "y")]).unwrap();
        assert!(!out.contains("turn"), "{out}");
    }

    #[test]
    fn a_removed_line_is_named_by_where_it_was() {
        let out = compose(
            "",
            &[on_line(
                DiffLineKind::Removed,
                41,
                "pub fn new() -> Self {",
                "why?",
            )],
        )
        .unwrap();
        assert!(
            out.contains("`src/widget.rs` — removed line, was 41: `pub fn new() -> Self {`"),
            "{out}"
        );
    }

    #[test]
    fn an_empty_comment_is_dropped() {
        let out = compose("hi", &[on_line(DiffLineKind::Added, 1, "a", "   ")]).unwrap();
        assert_eq!(out, "hi");
    }

    #[test]
    fn a_paragraph_stays_inside_its_item() {
        let out = compose(
            "",
            &[on_line(DiffLineKind::Context, 2, "a", "first\nsecond")],
        )
        .unwrap();
        assert!(out.ends_with("  first\n  second"), "{out}");
    }

    #[test]
    fn backticks_in_the_line_are_quoted_safely() {
        assert_eq!(code("a `b` c"), "``a `b` c``");
        assert_eq!(code("echo `date`"), "`` echo `date` ``");
        assert_eq!(code("`x`"), "`` `x` ``");
        assert_eq!(code(""), "(blank line)");
    }

    #[test]
    fn a_long_line_is_cut_on_a_character() {
        let long = "é".repeat(MAX_QUOTE + 10);
        let out = compose("", &[on_line(DiffLineKind::Added, 1, &long, "shorter")]).unwrap();
        assert!(
            out.contains(&format!("{}…", "é".repeat(MAX_QUOTE))),
            "{out}"
        );
    }

    #[test]
    fn too_many_comments_are_refused_not_cut() {
        let many: Vec<_> = (0..=MAX_REMARKS as u32)
            .map(|i| on_line(DiffLineKind::Added, i, "a", "b"))
            .collect();
        assert!(compose("", &many).is_err());
    }
}
