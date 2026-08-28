//! What the person had on screen when they sent a message.
//!
//! A sibling of [`crate::attach`], and the same kind of thing: something the
//! user had in front of them that is not the words they typed, turned into
//! something the model can read. A pasted screenshot is one; the dataset open
//! in the Data pane is another; the file open in the canvas, and the lines
//! selected in it, are the third.
//!
//! # Why this exists at all
//!
//! The composer sits below the Data pane as well as below the transcript, so
//! "which category refunds most?" is a question somebody asks while looking at
//! a table. Without this it is a question with no subject: the model has to
//! guess which of four loaded datasets was meant, or spend a turn asking. With
//! it, "this" has a referent.
//!
//! The canvas makes the same argument twice as loudly, because a document has
//! a *place* in it as well as an identity. "Tighten this paragraph" typed with
//! three paragraphs selected is a complete instruction to anyone looking at the
//! screen and an unanswerable one to anybody else. The selection is the whole
//! of what makes it answerable, so the selection travels.
//!
//! # Where it goes, and what that costs
//!
//! Onto the *prompt*, not onto the transcript's copy of what was said — the
//! same split `/command` expansion already makes, and for the same reason: this
//! is how the request is carried out rather than what was asked. The user's own
//! line stays their own line.
//!
//! The cost is that a reopened conversation does not show it. A message reading
//! "which category refunds most?" is complete when it is sent, because the chip
//! on the composer says which dataset it means, and incomplete a week later.
//! What makes that bearable is that the answer beneath it names the dataset;
//! what would fix it is a transcript entry that can carry more than text, which
//! is a larger change than this is. See `docs/known-gaps.md`.
//!
//! # Phrasing
//!
//! Written as a *moment* — "was open when this was sent" — rather than as a
//! standing fact. Every message from the pane carries one, so a conversation
//! accumulates them, and an older one saying "the pane was open on X" is true
//! about that message and would be a lie about the turn in progress.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The SQL in the query box, at most this many characters.
///
/// Generous, because the case this exists for is "why does this not work?" and
/// a query somebody is stuck on is exactly the long one. Capped anyway: the box
/// takes a paste, and a paste has no upper bound.
const MAX_SQL: usize = 2_000;

/// The selected passage, at most this many characters.
///
/// Larger than [`MAX_SQL`] because the thing being selected is prose or a
/// function rather than a query, and the request that carries one — "rewrite
/// this", "does this handle the empty case?" — is exactly as useful over forty
/// lines as over four. Still capped: ⌘A is one keystroke away in every editor
/// ever written, and a selection is under nobody's control but the user's.
const MAX_SELECTION: usize = 6_000;

/// What the person had in front of them, in whichever surface they were in.
///
/// Both halves are optional and both can be absent — a message sent from the
/// transcript carries an empty one, which [`OnScreen::describe`] turns into
/// nothing at all. They are not mutually exclusive on purpose: the two panes
/// are a split, so a question can genuinely be asked with a dataset and a file
/// both on screen, and picking one for the user would be guessing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OnScreen {
    /// The Data pane, when that is what was open.
    #[ts(optional)]
    pub data: Option<DataOnScreen>,
    /// The canvas, when a file was open in it.
    #[ts(optional)]
    pub document: Option<DocumentOnScreen>,
}

/// What the Data pane was showing.
///
/// Only the handle and the box. Not the columns — the model has
/// `profile_dataset` for those, and a forty-column listing on every message is
/// a real cost for something it can ask for. Not the rows, ever.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DataOnScreen {
    /// The selected dataset, as `load_dataset` named it.
    pub dataset: String,
    /// Its path, workspace-relative, so the model can say which file it means.
    pub path: String,
    /// Whatever was in the query box, when there was anything.
    #[ts(optional)]
    pub sql: Option<String>,
}

/// What the canvas was showing.
///
/// The path and the selection, and deliberately not the file's text. The model
/// has `read_file`, the canvas reads the same bytes off the same disk, and a
/// whole file on every message from a pane somebody leaves open would be the
/// most expensive habit in the app. The selection is the exception because it
/// is the part that *cannot* be looked up: nothing on disk records which forty
/// lines somebody had highlighted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentOnScreen {
    /// Workspace-relative, so the model can name the file back.
    pub path: String,
    /// What was highlighted, when anything was.
    #[ts(optional)]
    pub selection: Option<Selection>,
    /// Whether the editor holds something the file on disk does not.
    ///
    /// Nearly always false, because the canvas saves itself a moment after
    /// typing stops. What makes it worth a field anyway is the case where it
    /// stays true indefinitely: a save refused because somebody else wrote the
    /// file, where the screen and the disk hold different things until a person
    /// decides. Asking about the document then, without this, gets an answer
    /// about a version nobody is looking at — confidently, and with no sign
    /// anything is wrong.
    #[serde(default)]
    pub unsaved: bool,
}

/// A highlighted passage, and where in the file it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Selection {
    /// First line of the selection, counting from 1.
    pub from: u32,
    /// Last line, included.
    pub to: u32,
    /// The text itself, exactly as it reads in the file.
    pub text: String,
}

impl OnScreen {
    /// What the model is told, or nothing when there is nothing worth saying.
    ///
    /// Both halves, when both are there, separated by a blank line. Neither is
    /// abridged in the presence of the other: a split screen is a split screen,
    /// and a paragraph about the file is not made less true by a table beside
    /// it.
    pub fn describe(&self) -> Option<String> {
        let parts: Vec<String> = [
            self.data.as_ref().and_then(DataOnScreen::describe),
            self.document.as_ref().and_then(DocumentOnScreen::describe),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }
}

impl DataOnScreen {
    fn describe(&self) -> Option<String> {
        let dataset = self.dataset.trim();
        if dataset.is_empty() {
            return None;
        }
        let mut out = format!(
            "The Data pane was open on the dataset `{dataset}` ({}) when this message was sent. \
             Unless the message names another, \"this\", \"the data\", and \"the dataset\" mean \
             that one.",
            self.path.trim()
        );

        if let Some(sql) = self.sql.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            out.push_str("\n\nThe query box held:\n\n");
            out.push_str(&cut_to(sql, MAX_SQL, "the box holds more"));
        }
        Some(out)
    }
}

impl DocumentOnScreen {
    fn describe(&self) -> Option<String> {
        let path = self.path.trim();
        if path.is_empty() {
            return None;
        }

        // Said whether or not there is a selection, and said early: everything
        // after it is about a document whose contents on disk are not the
        // contents on screen, and a model that reads the file has been warned.
        let stale = if self.unsaved {
            " The editor holds unsaved changes, so the file on disk is **not** what is on \
             screen — say so rather than answering from what read_file returns."
        } else {
            ""
        };

        // Two sentences with and without a selection rather than one with a
        // clause bolted on, because the *referent* differs. With lines
        // highlighted, "this" is the passage; without, it is the file.
        let Some(at) = self
            .selection
            .as_ref()
            .filter(|s| !s.text.trim().is_empty())
        else {
            return Some(format!(
                "The file `{path}` was open in the editor when this message was sent. Unless the \
                 message names another, \"this\", \"the file\", and \"the document\" mean that \
                 one. Nothing was selected in it.{stale} Read it with read_file before answering \
                 anything about what it says."
            ));
        };

        let lines = if at.from == at.to {
            format!("line {}", at.from)
        } else {
            format!("lines {}–{}", at.from, at.to)
        };
        Some(format!(
            "The file `{path}` was open in the editor when this message was sent, with {lines} \
             selected.{stale} Unless the message names something else, \"this\", \"this \
             section\", and \"the selection\" mean the selected passage — which reads, in \
             full:\n\n{}",
            cut_to(&at.text, MAX_SELECTION, "the selection runs on")
        ))
    }
}

/// `text` if it fits, or its first `max` characters and a line saying so.
///
/// One function for both halves because they had the same three lines and the
/// same off-by-one to get wrong. Counted in characters rather than bytes: this
/// is cut for a reading budget, and cutting a multi-byte character in half
/// would produce something that is not text at all.
fn cut_to(text: &str, max: usize, more: &str) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str(&format!("\n… (cut here; {more})"));
    out
}

/// A prompt with what was on screen appended, or the prompt unchanged.
///
/// Appended rather than prepended. The message is the request and the pane is
/// the circumstance, and a model that reads the circumstance first will
/// sometimes answer the circumstance.
pub fn with_context(prompt: &str, on_screen: Option<&OnScreen>) -> String {
    match on_screen.and_then(OnScreen::describe) {
        Some(context) => format!("{prompt}\n\n---\n\n{context}"),
        None => prompt.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looking() -> OnScreen {
        OnScreen {
            data: Some(DataOnScreen {
                dataset: "interactions".into(),
                path: "data/interactions.csv".into(),
                sql: None,
            }),
            document: None,
        }
    }

    fn data(on: &mut OnScreen) -> &mut DataOnScreen {
        on.data.as_mut().unwrap()
    }

    fn reading() -> OnScreen {
        OnScreen {
            data: None,
            document: Some(DocumentOnScreen {
                path: "docs/known-gaps.md".into(),
                selection: None,
                unsaved: false,
            }),
        }
    }

    #[test]
    fn the_dataset_and_its_file_are_both_named() {
        let text = looking().describe().unwrap();
        assert!(text.contains("`interactions`"), "{text}");
        assert!(text.contains("data/interactions.csv"), "{text}");
    }

    /// The whole point: giving "this" a referent.
    #[test]
    fn it_says_what_this_refers_to() {
        assert!(looking().describe().unwrap().contains("\"this\""));
    }

    /// A moment, not a standing fact — every message from the pane carries one,
    /// and an older one must not read as a claim about the turn in progress.
    #[test]
    fn it_is_phrased_as_when_the_message_was_sent() {
        assert!(looking()
            .describe()
            .unwrap()
            .contains("when this message was sent"));
    }

    #[test]
    fn an_empty_query_box_adds_nothing() {
        let mut on = looking();
        data(&mut on).sql = Some("   \n ".into());
        assert!(!on.describe().unwrap().contains("query box"));
    }

    #[test]
    fn a_query_in_the_box_travels_with_it() {
        let mut on = looking();
        data(&mut on).sql = Some("SELECT count(*) FROM interactions".into());
        let text = on.describe().unwrap();
        assert!(text.contains("query box"), "{text}");
        assert!(text.contains("SELECT count(*)"), "{text}");
    }

    #[test]
    fn a_pasted_novel_is_cut_and_says_so() {
        let mut on = looking();
        data(&mut on).sql = Some("x".repeat(MAX_SQL * 2));
        let text = on.describe().unwrap();
        assert!(text.contains("the box holds more"), "{text}");
        assert!(text.chars().count() < MAX_SQL * 2);
    }

    /// Nothing selected is nothing to say, rather than a sentence about an
    /// empty pane.
    #[test]
    fn no_dataset_means_no_context() {
        let mut on = looking();
        data(&mut on).dataset = String::new();
        assert!(on.describe().is_none());
        assert_eq!(with_context("hello", Some(&on)), "hello");
    }

    #[test]
    fn a_message_sent_from_the_transcript_is_left_exactly_as_it_was() {
        assert_eq!(with_context("hello", None), "hello");
        assert!(OnScreen::default().describe().is_none());
    }

    /// The request first, the circumstance after: a model that reads the
    /// circumstance first will sometimes answer the circumstance.
    #[test]
    fn the_message_comes_before_the_context() {
        let full = with_context("which category refunds most?", Some(&looking()));
        assert!(full.starts_with("which category refunds most?"), "{full}");
        assert!(full.contains("Data pane was open"), "{full}");
    }

    /* ------------------------------------------------------------- canvas */

    #[test]
    fn an_open_file_is_named_and_this_is_pointed_at_it() {
        let text = reading().describe().unwrap();
        assert!(text.contains("`docs/known-gaps.md`"), "{text}");
        assert!(text.contains("\"this\""), "{text}");
        assert!(text.contains("when this message was sent"), "{text}");
    }

    /// The half that keeps a wrong answer from being confident: with nothing
    /// highlighted the model has been given a *name*, not the contents, and it
    /// has to be told to go and read them.
    #[test]
    fn a_file_with_nothing_selected_says_to_read_it() {
        let text = reading().describe().unwrap();
        assert!(text.contains("Nothing was selected"), "{text}");
        assert!(text.contains("read_file"), "{text}");
    }

    /// The reason the selection travels at all: nothing on disk records which
    /// lines somebody had highlighted, so this is the one thing here that
    /// cannot be looked up.
    #[test]
    fn a_selection_travels_with_its_line_numbers_and_its_text() {
        let mut on = reading();
        on.document.as_mut().unwrap().selection = Some(Selection {
            from: 40,
            to: 58,
            text: "the retry backs off exponentially".into(),
        });
        let text = on.describe().unwrap();
        assert!(text.contains("lines 40–58"), "{text}");
        assert!(text.contains("the retry backs off exponentially"), "{text}");
        assert!(text.contains("\"this section\""), "{text}");
        // The referent moved to the passage, so the instruction to go and read
        // the file would now be wrong.
        assert!(!text.contains("read_file"), "{text}");
    }

    /// One line reads as one line, not as a range from itself to itself.
    #[test]
    fn a_one_line_selection_says_line_rather_than_lines() {
        let mut on = reading();
        on.document.as_mut().unwrap().selection = Some(Selection {
            from: 12,
            to: 12,
            text: "const MAX: usize = 4;".into(),
        });
        let text = on.describe().unwrap();
        assert!(text.contains("line 12 selected"), "{text}");
        assert!(!text.contains("–"), "{text}");
    }

    /// ⌘A is one keystroke, so the cap is not theoretical.
    #[test]
    fn selecting_the_whole_file_is_cut_and_says_so() {
        let mut on = reading();
        on.document.as_mut().unwrap().selection = Some(Selection {
            from: 1,
            to: 9_000,
            text: "x".repeat(MAX_SELECTION * 2),
        });
        let text = on.describe().unwrap();
        assert!(text.contains("the selection runs on"), "{text}");
        assert!(text.chars().count() < MAX_SELECTION * 2);
    }

    /// A selection of blank lines is a drag that landed on nothing, and a
    /// paragraph quoting whitespace back is worse than the sentence about the
    /// file it replaced.
    #[test]
    fn a_selection_of_nothing_falls_back_to_naming_the_file() {
        let mut on = reading();
        on.document.as_mut().unwrap().selection = Some(Selection {
            from: 3,
            to: 4,
            text: "  \n \n".into(),
        });
        let text = on.describe().unwrap();
        assert!(text.contains("Nothing was selected"), "{text}");
    }

    /// The silent-wrong-answer case. A conflict leaves the screen and the disk
    /// holding different things until somebody decides, and a model answering
    /// from `read_file` in that window is answering about a version nobody is
    /// looking at.
    #[test]
    fn an_unsaved_buffer_warns_that_the_file_is_not_what_is_on_screen() {
        let mut on = reading();
        on.document.as_mut().unwrap().unsaved = true;
        let text = on.describe().unwrap();
        assert!(text.contains("unsaved"), "{text}");
        assert!(text.contains("not** what is on screen"), "{text}");

        // And with a selection too, where the passage quoted below it is the
        // unsaved one.
        on.document.as_mut().unwrap().selection = Some(Selection {
            from: 4,
            to: 4,
            text: "half a sentence".into(),
        });
        let text = on.describe().unwrap();
        assert!(text.contains("unsaved"), "{text}");
        assert!(text.contains("half a sentence"), "{text}");
    }

    /// Which is the ordinary case: the canvas saves a moment after typing
    /// stops, so a sentence about staleness would be wrong nearly every time.
    #[test]
    fn a_saved_buffer_says_nothing_about_staleness() {
        assert!(!reading().describe().unwrap().contains("unsaved"));
    }

    #[test]
    fn an_unnamed_file_is_nothing_to_say() {
        let mut on = reading();
        on.document.as_mut().unwrap().path = "  ".into();
        assert!(on.describe().is_none());
    }

    /// The panes are a split, so both can genuinely be on screen. Picking one
    /// would be the app guessing which the question was about.
    #[test]
    fn a_dataset_and_a_document_both_open_are_both_described() {
        let on = OnScreen {
            data: looking().data,
            document: reading().document,
        };
        let text = on.describe().unwrap();
        assert!(text.contains("Data pane was open"), "{text}");
        assert!(text.contains("was open in the editor"), "{text}");
    }
}
