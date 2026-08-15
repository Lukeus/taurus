//! What a tool call asks the transcript to draw.
//!
//! Almost every tool reports what it *did*, and a line in the run header is the
//! right size for that. Three of them report something to *look at* — a table,
//! a chart, a question — and a line cannot hold any of those. So those hand the
//! frontend a payload and let it draw.
//!
//! The payload lives here, in the lowest crate a tool and the agent loop can
//! both see, for the same reason [`crate::tool::ToolProgress`] takes a rendered
//! line rather than a UI event type: a tool that wants a table drawn should not
//! have to know that tables are drawn in React, in a terminal, or at all.
//!
//! These types are the tools' input schemas as well as their output payloads.
//! That is deliberate and load-bearing: a saved conversation records the call's
//! input and nothing about how it was rendered, so a view that is exactly its
//! own input is a view a reopened conversation can redraw. Anything derived
//! along the way would be lost the moment the app restarted.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A view to draw in place of a tool call's row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum TranscriptView {
    Table {
        title: String,
        /// Where the numbers came from. Shown under the title.
        caption: Option<String>,
        columns: Vec<Column>,
        /// Cells as written, one inner vector per row, in column order.
        rows: Vec<Vec<String>>,
    },
    Chart {
        title: String,
        caption: Option<String>,
        /// One label per bar, shared by every series.
        labels: Vec<String>,
        series: Vec<Series>,
    },
    Questions {
        /// The tool call waiting on these. An answer quotes it back, which is
        /// how the harness knows which blocked call to resume.
        id: String,
        questions: Vec<Question>,
    },
}

/// One column of a table, and how to read the cells under it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct Column {
    pub label: String,
    #[serde(default)]
    pub kind: ColumnKind,
}

/// What a column holds. Decides alignment, and how a click on the header sorts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ColumnKind {
    /// Words. Left-aligned, sorted alphabetically.
    #[default]
    Text,
    /// A quantity. Right-aligned, sorted by value rather than by digit.
    Number,
    /// A change, where the sign is the point: `+22%`, `-8`, `0`. Sorted as a
    /// number and tinted by direction, so a regression is visible without
    /// reading the column.
    Delta,
}

/// One line of a chart — a metric that can be plotted against the shared
/// labels. More than one becomes a row of tabs over a single set of bars.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct Series {
    /// Names the metric, e.g. `tool calls`. Doubles as the tab label.
    pub name: String,
    /// Suffix for every value, e.g. `s` or `k`. Empty for a bare count.
    #[serde(default)]
    pub unit: String,
    /// One value per label, in the same order.
    pub values: Vec<f64>,
}

/// A single question put to the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct Question {
    /// The question itself, as a sentence.
    pub prompt: String,
    #[serde(default)]
    pub kind: QuestionKind,
    pub options: Vec<QuestionOption>,
    /// Whether a free-text box appears under the options. Offer it when the
    /// options might not cover the answer.
    #[serde(default)]
    pub allow_other: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum QuestionKind {
    /// Exactly one option, or none.
    #[default]
    Single,
    /// Any number of options, including none.
    Multi,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct QuestionOption {
    /// What the option says. This is the text you get back, so make each one
    /// stand on its own.
    pub label: String,
    /// The trade-off, in a few words — `2 files`, `safest`, `slower to build`.
    /// Shown beside the label in a dimmer face.
    #[serde(default)]
    pub note: String,
}

/// What the user chose for one question, positionally matched to the question
/// it answers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Answer {
    /// Option labels chosen. Empty when the question was skipped, which is
    /// allowed for every question.
    pub picked: Vec<String>,
    /// What they typed instead, where the question offered a free-text box.
    pub other: Option<String>,
}

impl Answer {
    /// Whether the user said anything at all here.
    pub fn is_empty(&self) -> bool {
        self.picked.is_empty() && self.other.as_deref().unwrap_or("").trim().is_empty()
    }

    /// The answer as one line, for the tool result and for the terminal.
    pub fn render(&self) -> String {
        let mut parts = self.picked.clone();
        if let Some(other) = self
            .other
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(other.to_string());
        }
        parts.join(", ")
    }
}

/// Puts questions to whoever is watching, and waits.
///
/// The narrower sibling of [`crate::permission::PermissionPrompt`], and blocking
/// for the same reason: an answer that arrives after the model has moved on is
/// not an answer. Implemented by each frontend — a card in the transcript, a
/// prompt on the terminal — and by nobody at all in a piped run, which is what
/// [`Unattended`] is for.
#[async_trait::async_trait]
pub trait Asker: Send + Sync {
    /// One answer per question, in order. `None` when there is nobody to ask,
    /// which the caller must treat as "decide for yourself" rather than as an
    /// error: an unattended run has to keep going.
    async fn ask(&self, id: &str, questions: &[Question]) -> Option<Vec<Answer>>;
}

/// Answers nothing, immediately. For piped runs, examples, and tests, where a
/// prompt would hang forever with nobody to see it.
pub struct Unattended;

#[async_trait::async_trait]
impl Asker for Unattended {
    async fn ask(&self, _id: &str, _questions: &[Question]) -> Option<Vec<Answer>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_reads_as_one_line_whatever_it_holds() {
        let picked = Answer {
            picked: vec!["Tests".into(), "TypeScript bindings".into()],
            other: None,
        };
        assert_eq!(picked.render(), "Tests, TypeScript bindings");

        let typed = Answer {
            picked: Vec::new(),
            other: Some("  just the changelog  ".into()),
        };
        assert_eq!(typed.render(), "just the changelog");
        assert!(!typed.is_empty());
    }

    #[test]
    fn whitespace_alone_is_not_an_answer() {
        // The free-text box is always present when offered, so an untouched one
        // arrives as an empty string — and a question the user skipped must not
        // come back looking answered.
        let blank = Answer {
            picked: Vec::new(),
            other: Some("   ".into()),
        };
        assert!(blank.is_empty());
        assert_eq!(blank.render(), "");
    }
}
