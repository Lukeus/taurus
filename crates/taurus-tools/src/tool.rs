//! The tool trait, its error type, and the context handed to every call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::diff::FileDiff;
use crate::permission::PermissionEngine;

/// What a tool does to the world. Drives the permission tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Effect {
    /// Observes without changing anything. Auto-allowed inside the workspace.
    Read,
    /// Creates, modifies, or deletes files.
    Write,
    /// Runs a program.
    Execute,
    /// Reaches outside the machine.
    Network,
}

impl Effect {
    /// Read-only tools are safe to run concurrently within one turn; anything
    /// else is serialized so ordering stays deterministic.
    pub fn is_concurrent_safe(self) -> bool {
        self == Effect::Read
    }
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("{path} is outside the workspace ({root})")]
    OutsideWorkspace { path: String, root: String },

    #[error("permission denied by the user")]
    Denied,

    #[error("no such tool: {0}")]
    NotFound(String),

    #[error("{0}")]
    Failed(String),

    #[error("canceled")]
    Canceled,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl ToolError {
    /// A stable, low-cardinality name for this kind of failure, for
    /// `error.type` on a span. See [`taurus_provider::ProviderError::kind`].
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::OutsideWorkspace { .. } => "outside_workspace",
            Self::Denied => "denied",
            Self::NotFound(_) => "not_found",
            Self::Failed(_) => "failed",
            Self::Canceled => "canceled",
            Self::Io(_) => "io",
        }
    }

    /// Rendered back to the model as an error tool result. Phrased so the model
    /// can act on it rather than just apologize.
    pub fn to_model_message(&self) -> String {
        match self {
            Self::InvalidInput(m) => {
                format!("Invalid input: {m}. Check the tool's schema and retry.")
            }
            Self::OutsideWorkspace { path, root } => format!(
                "Refused: {path} is outside the workspace ({root}). Only paths under the \
                 workspace root can be accessed."
            ),
            Self::Denied => {
                "The user denied this action. Do not retry it; choose another approach or ask \
                 what they would prefer."
                    .into()
            }
            Self::NotFound(name) => {
                format!("No tool named '{name}' exists. Use only the tools listed for you.")
            }
            other => other.to_string(),
        }
    }
}

/// What a tool call produced, or why it did not.
///
/// The success side is a block list rather than a string because some answers
/// are not text: a screenshot, a rendered chart, a page of a PDF. `From<String>`
/// and `From<&str>` are implemented, so a tool with nothing but prose to return
/// still writes `Ok("...".into())` and never thinks about blocks.
pub type ToolResult = Result<taurus_provider::ToolOutput, ToolError>;

/// Where a tool reports what it is doing before it has a result.
///
/// Only worth implementing for calls long enough that silence is ambiguous. A
/// sub-agent can work for a minute, and a card that says "running" for that
/// long is indistinguishable from one that has hung.
///
/// Deliberately one method taking a rendered line, rather than the UI event
/// type: that lives in `taurus-core`, which depends on this crate, and a tool
/// reporting progress does not need to know how progress is drawn.
#[async_trait]
pub trait ToolProgress: Send + Sync {
    async fn step(&self, label: String);

    /// This call has a conversation of its own, kept under `session`.
    ///
    /// Delegation is the only caller: a sub-agent's transcript is written
    /// somewhere the parent's card can offer to open, and the id is how it
    /// finds it. Announced when the child *starts*, so a delegation that is
    /// still running or was canceled can be read too — those being the ones
    /// somebody most wants to read.
    ///
    /// Defaulted to nothing, because it is nothing for every other tool.
    async fn transcript(&self, session: String, agent: String) {
        let _ = (session, agent);
    }
}

/// Everything a tool needs at call time.
#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    /// Directories read-only tools may reach into besides the workspace.
    ///
    /// In practice the loaded skills' own directories, so a procedure that says
    /// "see references/REFERENCE.md" can be followed when the skill lives under
    /// the home directory. Reads only — nothing here widens what may be
    /// written, and the workspace remains the only place this agent changes.
    pub readable_roots: Vec<PathBuf>,
    pub permissions: Arc<PermissionEngine>,
    pub cancel: CancellationToken,
    /// The open turn that file changes are checkpointed into.
    ///
    /// Optional because not every caller wants a rewindable turn — a piped run
    /// that only reads, an example, a test. Cloning the context shares the
    /// recorder, which is how a sub-agent's writes land in the turn that
    /// spawned it.
    pub checkpoints: Option<Arc<crate::checkpoint::TurnRecorder>>,
    /// What the last command in this turn read of the workspace, so the next
    /// one need not read it again. See [`crate::sweep::SweepCache`].
    ///
    /// Opened and closed with `checkpoints` because it has no other use: a
    /// sweep only runs when there is a turn to record it into. Shared by a
    /// clone of this context for the same reason the recorder is — a
    /// sub-agent's commands sweep the same workspace, and reading it a second
    /// time on their behalf would answer the same question twice.
    pub sweeps: Option<Arc<crate::sweep::SweepCache>>,
    /// Bound to one tool call by the agent loop, so a report lands on the right
    /// card. `None` outside a loop that draws anything — the CLI's piped mode,
    /// examples, tests.
    pub progress: Option<Arc<dyn ToolProgress>>,
    /// The user's configured hooks, if any.
    ///
    /// `None` outside a harness that loaded config — an example, a test, a
    /// caller running one tool directly. Shared by a clone of this context, so
    /// a sub-agent's calls go through the same hooks the parent's do: a guard
    /// that a delegate could route around is not a guard.
    pub hooks: Option<Arc<taurus_hooks::HookRunner>>,
    /// The conversation these calls belong to, passed to hooks so one can tell
    /// two sessions apart. `None` wherever there is no session.
    pub session_id: Option<String>,
    /// The id of the call this context was built for.
    ///
    /// Only [`crate::builtin::present::AskUser`] reads it, and only because it
    /// has to: the question card the user is looking at was drawn from this id,
    /// so the wait for their answer has to be registered under the same one.
    /// `None` wherever the caller runs a tool outside the agent loop.
    pub call_id: Option<String>,
}

impl ToolContext {
    pub fn new(
        workspace: impl Into<PathBuf>,
        permissions: Arc<PermissionEngine>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            readable_roots: Vec::new(),
            permissions,
            cancel,
            checkpoints: None,
            sweeps: None,
            progress: None,
            hooks: None,
            session_id: None,
            call_id: None,
        }
    }

    /// Attaches the configured hooks.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<taurus_hooks::HookRunner>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Names the conversation, for hooks that care which one they are in.
    #[must_use]
    pub fn with_session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Widens what read-only tools may open, without widening what may change.
    pub fn with_readable_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.readable_roots = roots;
        self
    }

    /// Makes changes made through this context undoable.
    pub fn with_checkpoints(mut self, recorder: Arc<crate::checkpoint::TurnRecorder>) -> Self {
        self.checkpoints = Some(recorder);
        // Together, always. Every caller that opens a turn wants both, and one
        // without the other is either a sweep with nowhere to record or a turn
        // that re-reads the workspace before every command it runs.
        self.sweeps = Some(Arc::new(crate::sweep::SweepCache::new()));
        self
    }

    /// Binds this context to one tool call's progress reporting.
    pub fn with_progress(mut self, progress: Arc<dyn ToolProgress>) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Tells the call which call it is. See [`ToolContext::call_id`].
    pub fn with_call_id(mut self, id: impl Into<String>) -> Self {
        self.call_id = Some(id.into());
        self
    }

    /// Reports a step, if anyone is listening. Costs nothing when nobody is.
    pub async fn report(&self, label: impl Into<String>) {
        if let Some(progress) = &self.progress {
            progress.step(label.into()).await;
        }
    }

    /// Says where this call's own conversation is being kept. See
    /// [`ToolProgress::transcript`].
    pub async fn report_transcript(&self, session: impl Into<String>, agent: impl Into<String>) {
        if let Some(progress) = &self.progress {
            progress.transcript(session.into(), agent.into()).await;
        }
    }

    /// Resolves a path a tool is about to change. Workspace only.
    pub fn resolve(&self, candidate: &str) -> Result<PathBuf, ToolError> {
        crate::path_guard::resolve(&self.workspace, candidate)
    }

    /// Resolves a path a tool is only going to read, which may also sit in one
    /// of the skill directories the session loaded.
    pub fn resolve_read(&self, candidate: &str) -> Result<PathBuf, ToolError> {
        crate::path_guard::resolve_within(&self.workspace, &self.readable_roots, candidate)
    }

    pub fn display(&self, path: &Path) -> String {
        crate::path_guard::display(&self.workspace, path)
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// Shown to the model. This is prompt text and the model's only guidance on
    /// when to reach for the tool, so it should read as instructions.
    fn description(&self) -> &str;

    /// JSON Schema for the input object.
    fn input_schema(&self) -> serde_json::Value;

    fn effect(&self) -> Effect;

    /// One line describing what this specific call will do, shown in the
    /// permission prompt. The user is approving *this* call, not the tool in
    /// general, so it must reflect the arguments.
    fn preview(&self, input: &serde_json::Value) -> String {
        format!("{} {}", self.name(), compact(input))
    }

    /// The change this call would make to a file, when the tool can work one out.
    ///
    /// The default is `None`, which is right for every tool whose effect is not
    /// a file rewrite. `write_file` and `edit_file` override it, because for
    /// those the one-line preview names a file and a byte count and stops
    /// exactly where the interesting part begins.
    ///
    /// Given the workspace rather than a [`ToolContext`] because this runs
    /// inside the permission gate, before a context exists for the call — and
    /// because the only thing it needs from one is the root to resolve against.
    /// Returning `None` on any failure is deliberate: a diff is evidence
    /// offered alongside the decision, never a precondition for making it, so a
    /// file that cannot be read still gets a prompt with the preview on it.
    async fn diff(&self, _input: &serde_json::Value, _workspace: &Path) -> Option<FileDiff> {
        None
    }

    /// Files this call may change, as the caller wrote them.
    ///
    /// Read just before the call so a rewind has something to put back. The
    /// default is empty, which means *this tool changes nothing, or nothing
    /// knowable in advance*. Tools of the second kind say so with
    /// [`Tool::touches_unpredictably`].
    fn touches(&self, _input: &serde_json::Value) -> Vec<String> {
        Vec::new()
    }

    /// Whether this tool can change files in the workspace without being able
    /// to name them first.
    ///
    /// `run_command` is the case: a command line does not say what it will
    /// rewrite, and a guess would be worse than no answer. Declaring this puts
    /// the call inside [`crate::sweep`], which looks at the workspace before
    /// and after instead of asking the tool to predict itself.
    ///
    /// Deliberately not inferred from [`Effect::Execute`]. That is a permission
    /// tier — MCP tools take it because an external program is doing arbitrary
    /// work, not because it is doing it to these files — and reading it as a
    /// statement about the filesystem would make every call to a remote API
    /// index the entire workspace twice.
    fn touches_unpredictably(&self) -> bool {
        false
    }

    /// What this call wants drawn in the transcript, instead of a row saying it
    /// happened.
    ///
    /// `None` for every tool that does work, which is nearly all of them — a
    /// row is the right size for "read 459 lines". The three in
    /// [`crate::builtin::present`] answer with a table, a chart, or a question
    /// card, because none of those fits on a line.
    ///
    /// Called before the tool runs, from the raw input, so a view appears the
    /// moment the call is announced rather than after it finishes. `id` is the
    /// call's id, which a view that expects an answer back must carry.
    /// Unparseable input answers `None` and the call goes on to fail properly
    /// in `execute`, where the error message can say what was wrong with it.
    fn view(&self, _id: &str, _input: &serde_json::Value) -> Option<crate::view::TranscriptView> {
        None
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult;
}

/// Single-line rendering of tool input for previews and logs.
pub fn compact(value: &serde_json::Value) -> String {
    const MAX: usize = 120;
    let s = value.to_string();
    if s.chars().count() <= MAX {
        return s;
    }
    let truncated: String = s.chars().take(MAX).collect();
    format!("{truncated}…")
}

/// Deserializes tool input, turning serde's message into something the model
/// can correct on the next attempt.
pub fn parse_input<T: serde::de::DeserializeOwned>(
    input: serde_json::Value,
) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))
}

/// JSON Schema for a tool's input struct, with every nested type written out
/// where it is used.
///
/// `schemars` factors a nested struct into `$defs` and points at it with a
/// `$ref`, which is correct and unreadable. A model does not dereference: shown
/// `"items": {"$ref": "#/$defs/Step"}` it sees an array of *something* and
/// invents field names for it, which is a tool call that fails on a schema the
/// model was never actually told. Inlining costs a few duplicated bytes in the
/// one case a type is used twice and buys the model the field names it is being
/// asked to produce.
///
/// A type that contains itself cannot be written out this way, and `schemars`
/// overflows the stack rather than declining. Nothing here is recursive, and
/// `every_tool_advertises_a_schema_with_no_refs_left` fails loudly if that
/// changes.
pub fn schema_for<T: schemars::JsonSchema>() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::default().with(|s| {
        s.inline_subschemas = true;
    });
    let schema = settings.into_generator().into_root_schema_for::<T>();
    serde_json::to_value(schema).unwrap_or_else(|_| serde_json::json!({ "type": "object" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every position in `schema` where a `$ref` survived.
    fn refs(schema: &serde_json::Value) -> Vec<String> {
        match schema {
            serde_json::Value::Object(map) => map
                .iter()
                .flat_map(|(key, value)| match (key.as_str(), value.as_str()) {
                    ("$ref", Some(target)) => vec![target.to_string()],
                    _ => refs(value),
                })
                .collect(),
            serde_json::Value::Array(items) => items.iter().flat_map(refs).collect(),
            _ => Vec::new(),
        }
    }

    /// The four tools whose input holds a nested type, and so the only four
    /// that `schemars` would factor into `$defs` at all.
    ///
    /// A model does not resolve a `$ref`. When this failed, `update_plan` was
    /// advertising an array of `#/$defs/Step` and models were filling it with
    /// objects keyed on whatever they guessed a step was called.
    #[test]
    fn every_tool_advertises_a_schema_with_no_refs_left() {
        use crate::builtin::plan::UpdatePlanInput;
        use crate::builtin::present::{AskUserInput, ShowChartInput, ShowTableInput};

        for (name, schema) in [
            ("update_plan", schema_for::<UpdatePlanInput>()),
            ("show_table", schema_for::<ShowTableInput>()),
            ("show_chart", schema_for::<ShowChartInput>()),
            ("ask_user", schema_for::<AskUserInput>()),
        ] {
            assert!(
                refs(&schema).is_empty(),
                "{name} still points at {:?}; a model reads the schema it is sent and nothing else",
                refs(&schema)
            );
            assert!(
                schema.get("$defs").is_none(),
                "{name} carries a $defs nothing references"
            );
        }
    }

    #[test]
    fn an_inlined_schema_still_names_the_nested_fields() {
        // The specific thing the model was missing: `text` one level down.
        let schema = schema_for::<crate::builtin::plan::UpdatePlanInput>();
        let step = &schema["properties"]["steps"]["items"];
        assert_eq!(step["properties"]["text"]["type"], "string");
        assert_eq!(step["required"], serde_json::json!(["text"]));
    }
}
