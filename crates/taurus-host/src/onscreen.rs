//! What the person had on screen when they sent a message.
//!
//! A sibling of [`crate::attach`], and the same kind of thing: something the
//! user had in front of them that is not the words they typed, turned into
//! something the model can read. A pasted screenshot is one; the dataset open
//! in the Data pane is another.
//!
//! # Why this exists at all
//!
//! The composer sits below the Data pane as well as below the transcript, so
//! "which category refunds most?" is a question somebody asks while looking at
//! a table. Without this it is a question with no subject: the model has to
//! guess which of four loaded datasets was meant, or spend a turn asking. With
//! it, "this" has a referent.
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

/// What the Data pane was showing.
///
/// Only the handle and the box. Not the columns — the model has
/// `profile_dataset` for those, and a forty-column listing on every message is
/// a real cost for something it can ask for. Not the rows, ever.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OnScreen {
    /// The selected dataset, as `load_dataset` named it.
    pub dataset: String,
    /// Its path, workspace-relative, so the model can say which file it means.
    pub path: String,
    /// Whatever was in the query box, when there was anything.
    #[ts(optional)]
    pub sql: Option<String>,
}

impl OnScreen {
    /// What the model is told, or nothing when there is nothing worth saying.
    pub fn describe(&self) -> Option<String> {
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
            if sql.chars().count() > MAX_SQL {
                let cut: String = sql.chars().take(MAX_SQL).collect();
                out.push_str(&cut);
                out.push_str("\n… (cut here; the box holds more)");
            } else {
                out.push_str(sql);
            }
        }
        Some(out)
    }
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
            dataset: "interactions".into(),
            path: "data/interactions.csv".into(),
            sql: None,
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
        on.sql = Some("   \n ".into());
        assert!(!on.describe().unwrap().contains("query box"));
    }

    #[test]
    fn a_query_in_the_box_travels_with_it() {
        let mut on = looking();
        on.sql = Some("SELECT count(*) FROM interactions".into());
        let text = on.describe().unwrap();
        assert!(text.contains("query box"), "{text}");
        assert!(text.contains("SELECT count(*)"), "{text}");
    }

    #[test]
    fn a_pasted_novel_is_cut_and_says_so() {
        let mut on = looking();
        on.sql = Some("x".repeat(MAX_SQL * 2));
        let text = on.describe().unwrap();
        assert!(text.contains("the box holds more"), "{text}");
        assert!(text.chars().count() < MAX_SQL * 2);
    }

    /// Nothing selected is nothing to say, rather than a sentence about an
    /// empty pane.
    #[test]
    fn no_dataset_means_no_context() {
        let mut on = looking();
        on.dataset = String::new();
        assert!(on.describe().is_none());
        assert_eq!(with_context("hello", Some(&on)), "hello");
    }

    #[test]
    fn a_message_sent_from_the_transcript_is_left_exactly_as_it_was() {
        assert_eq!(with_context("hello", None), "hello");
    }

    /// The request first, the circumstance after: a model that reads the
    /// circumstance first will sometimes answer the circumstance.
    #[test]
    fn the_message_comes_before_the_context() {
        let full = with_context("which category refunds most?", Some(&looking()));
        assert!(full.starts_with("which category refunds most?"), "{full}");
        assert!(full.contains("Data pane was open"), "{full}");
    }
}
