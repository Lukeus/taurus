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
use serde_json::Value;
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
    /// The checklist the model is working through.
    ///
    /// The odd one out here: a table and a chart are shown to the person and
    /// then forgotten, while this is read back to the *model* on every
    /// iteration as well. See [`crate::plan`].
    Plan { steps: Vec<Step> },
}

// Doc comments on this struct and its fields are advertised to the model
// verbatim, so they say what a step is and nothing about how it is read. The
// note on how much `Deserialize` forgives lives on `from_json` below.
/// One item on the model's checklist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[ts(export)]
pub struct Step {
    /// What is to be done, as a short imperative — `Add the token type`.
    pub text: String,
    #[serde(default)]
    pub state: StepState,
    /// The same step as something in progress — `Adding the token type`.
    /// Optional, and only ever shown while this is the step being worked on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
}

/// Keys a model reaches for when it means `text`, best first.
///
/// Not a wish list. Every one of these is a name a model picked for the same
/// field when it was guessing, and guessing is the normal case for a nested
/// object: the schema names `text` one level down from the array, and plenty of
/// models read the array and stop.
const TEXT_KEYS: &[&str] = &[
    "text",
    "step",
    "task",
    "title",
    "name",
    "description",
    "content",
];

/// The same, for `state`.
const STATE_KEYS: &[&str] = &["state", "status"];

/// The same, for `active_form`.
///
/// `activeForm` is here because it is the name Claude Code's own checklist
/// tool uses, and a model that has seen that one reaches for it.
const ACTIVE_FORM_KEYS: &[&str] = &["active_form", "activeForm", "active_text"];

impl Step {
    /// Reads a step from whatever the model actually sent.
    ///
    /// The strict reading of this struct is one shape; a model that has not
    /// resolved the schema produces a dozen, and all of them are unambiguous to
    /// a reader. Refusing them spends an iteration re-deriving a plan that was
    /// already perfectly clear — the argument
    /// [`crate::builtin::plan`] already makes for defaulting an omitted state,
    /// applied to the field the state hangs off.
    ///
    /// This is not the same permissiveness the plan tool's own checks refuse.
    /// Those reject plans the model got *wrong* — two steps in
    /// progress, a step that is a paragraph — where quietly repairing it would
    /// hand the model back a checklist it did not write. Naming the field
    /// `task` instead of `text` is not a plan the model got wrong. It is the
    /// same plan, spelled differently.
    fn from_json(value: Value) -> Result<Self, String> {
        let mut map = match value {
            // A bare string is a step with nothing said about its state, which
            // is exactly what the state's default is for.
            Value::String(text) => {
                return Ok(Self {
                    text,
                    state: StepState::default(),
                    active_form: None,
                })
            }
            Value::Object(map) => map,
            other => {
                return Err(format!(
                    "a step should be {{\"text\": \"Add the token type\", \"state\": \"todo\"}}, \
                     or just the text on its own, not {}",
                    describe(&other)
                ))
            }
        };

        let text = TEXT_KEYS
            .iter()
            .find_map(|key| map.remove(*key))
            .ok_or_else(|| match map.keys().cloned().collect::<Vec<_>>() {
                keys if keys.is_empty() => {
                    "a step needs a `text` field saying what is to be done".to_string()
                }
                keys => format!(
                    "a step needs a `text` field saying what is to be done; this one has {}",
                    keys.join(", ")
                ),
            })?;
        let text = match text {
            Value::String(text) => text,
            other => {
                return Err(format!(
                    "a step's `text` should be a string, not {}",
                    describe(&other)
                ))
            }
        };

        let state = match STATE_KEYS.iter().find_map(|key| map.remove(*key)) {
            None | Some(Value::Null) => StepState::default(),
            Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
                format!("a step's `state` should be 'todo', 'active', or 'done', not {value}")
            })?,
        };

        // Absent and blank are the same thing here. A model that has nothing to
        // say in this field says it by sending "", and a summary line reading
        // as empty is worse than one that falls back to the imperative.
        let active_form = match ACTIVE_FORM_KEYS.iter().find_map(|key| map.remove(*key)) {
            None | Some(Value::Null) => None,
            Some(Value::String(form)) if form.trim().is_empty() => None,
            Some(Value::String(form)) => Some(form),
            Some(other) => {
                return Err(format!(
                    "a step's `active_form` should be a string, not {}",
                    describe(&other)
                ))
            }
        };

        Ok(Self {
            text,
            state,
            active_form,
        })
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_json(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// A JSON value in the words an error message can use.
fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a true/false",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StepState {
    /// Not started.
    #[default]
    #[serde(
        alias = "pending",
        alias = "not_started",
        alias = "open",
        alias = "waiting"
    )]
    Todo,
    /// Being worked on right now. At most one step may be this: a checklist
    /// with three things in progress is a list, not a plan, and it tells the
    /// reader — and the model reading it back — nothing about where it is.
    #[serde(
        alias = "in_progress",
        alias = "in-progress",
        alias = "doing",
        alias = "current",
        alias = "running"
    )]
    Active,
    /// Finished.
    #[serde(alias = "completed", alias = "complete", alias = "finished")]
    Done,
}

impl StepState {
    /// The marker this state wears in a plain-text rendering, sized so a
    /// column of them lines up.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Todo => "[ ]",
            Self::Active => "[>]",
            Self::Done => "[x]",
        }
    }
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

    fn step(value: serde_json::Value) -> Result<Step, String> {
        serde_json::from_value::<Step>(value).map_err(|e| e.to_string())
    }

    #[test]
    fn the_shape_the_schema_asks_for_reads_as_itself() {
        assert_eq!(
            step(serde_json::json!({
                "text": "Add the token type",
                "state": "active",
                "active_form": "Adding the token type",
            })),
            Ok(Step {
                text: "Add the token type".into(),
                state: StepState::Active,
                active_form: Some("Adding the token type".into()),
            })
        );
    }

    #[test]
    fn a_step_that_is_just_its_text_is_a_step() {
        // What a model sends when it reads `steps` as an array of strings.
        assert_eq!(
            step(serde_json::json!("Add the token type")),
            Ok(Step {
                text: "Add the token type".into(),
                state: StepState::Todo,
                active_form: None,
            })
        );
    }

    #[test]
    fn the_active_form_is_optional_and_taken_under_either_spelling() {
        // `activeForm` is what Claude Code's checklist tool calls it, so it is
        // the name a model is most likely to reach for unprompted.
        for key in ACTIVE_FORM_KEYS {
            assert_eq!(
                step(serde_json::json!({ "text": "Add it", *key: "Adding it" }))
                    .map(|s| s.active_form),
                Ok(Some("Adding it".to_string())),
                "for {key}"
            );
        }
        assert_eq!(
            step(serde_json::json!({ "text": "Add it" })).map(|s| s.active_form),
            Ok(None)
        );
    }

    #[test]
    fn a_blank_active_form_is_no_active_form() {
        // Otherwise the summary line above the list renders as empty, which
        // reads as a bug rather than as an omission.
        assert_eq!(
            step(serde_json::json!({ "text": "Add it", "active_form": "   " }))
                .map(|s| s.active_form),
            Ok(None)
        );
    }

    #[test]
    fn the_text_field_is_taken_under_any_of_its_usual_names() {
        // The live failure: `missing field \`text\`` on a plan that said
        // exactly what it meant under a different key.
        for key in TEXT_KEYS {
            assert_eq!(
                step(serde_json::json!({ *key: "Add the token type" })).map(|s| s.text),
                Ok("Add the token type".to_string()),
                "for {key}"
            );
        }
    }

    #[test]
    fn the_state_is_taken_under_its_usual_names_and_spellings() {
        for (sent, expected) in [
            ("todo", StepState::Todo),
            ("pending", StepState::Todo),
            ("active", StepState::Active),
            ("in_progress", StepState::Active),
            ("in-progress", StepState::Active),
            ("done", StepState::Done),
            ("completed", StepState::Done),
        ] {
            for key in STATE_KEYS {
                assert_eq!(
                    step(serde_json::json!({ "text": "One", *key: sent })).map(|s| s.state),
                    Ok(expected),
                    "for {key}={sent}"
                );
            }
        }
    }

    #[test]
    fn a_step_with_no_text_at_all_is_told_what_it_sent_instead() {
        // Refusal is still the answer when there is nothing to read as text.
        // What changes is that the model is told what it actually sent, rather
        // than the name of a field it never chose.
        let error = step(serde_json::json!({ "id": 3, "notes": "later" })).unwrap_err();
        assert!(error.contains("needs a `text` field"), "{error}");
        assert!(error.contains("id, notes"), "{error}");
    }

    #[test]
    fn a_state_that_is_not_one_of_the_three_names_them() {
        let error = step(serde_json::json!({ "text": "One", "state": "blocked" })).unwrap_err();
        assert!(error.contains("'todo', 'active', or 'done'"), "{error}");
    }

    #[test]
    fn a_step_that_is_neither_text_nor_an_object_says_so() {
        let error = step(serde_json::json!(7)).unwrap_err();
        assert!(error.contains("not a number"), "{error}");
    }

    #[test]
    fn what_a_step_deserializes_from_is_what_it_serializes_back_to() {
        // The forgiving reading only ever produces the canonical shape, so a
        // plan read from a sloppy call and a plan read from a saved
        // conversation are the same plan.
        let sloppy = step(serde_json::json!({
            "task": "One",
            "status": "in_progress",
            "activeForm": "Doing one",
        }))
        .unwrap();
        let written = serde_json::to_value(&sloppy).unwrap();
        assert_eq!(
            written,
            serde_json::json!({ "text": "One", "state": "active", "active_form": "Doing one" })
        );
        assert_eq!(step(written), Ok(sloppy));
    }

    #[test]
    fn a_step_without_an_active_form_does_not_write_the_field_at_all() {
        // Every plan is written into the saved conversation. A null per step
        // per rewrite is noise in a file that is read back on every reopen.
        let written = serde_json::to_value(step(serde_json::json!("One")).unwrap()).unwrap();
        assert_eq!(
            written,
            serde_json::json!({ "text": "One", "state": "todo" })
        );
    }
}
