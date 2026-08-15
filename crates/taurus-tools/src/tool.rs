//! The tool trait, its error type, and the context handed to every call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

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

pub type ToolResult = Result<String, ToolError>;

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
    /// Bound to one tool call by the agent loop, so a report lands on the right
    /// card. `None` outside a loop that draws anything — the CLI's piped mode,
    /// examples, tests.
    pub progress: Option<Arc<dyn ToolProgress>>,
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
            progress: None,
        }
    }

    /// Widens what read-only tools may open, without widening what may change.
    pub fn with_readable_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.readable_roots = roots;
        self
    }

    /// Makes changes made through this context undoable.
    pub fn with_checkpoints(mut self, recorder: Arc<crate::checkpoint::TurnRecorder>) -> Self {
        self.checkpoints = Some(recorder);
        self
    }

    /// Binds this context to one tool call's progress reporting.
    pub fn with_progress(mut self, progress: Arc<dyn ToolProgress>) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Reports a step, if anyone is listening. Costs nothing when nobody is.
    pub async fn report(&self, label: impl Into<String>) {
        if let Some(progress) = &self.progress {
            progress.step(label.into()).await;
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

/// JSON Schema for a tool's input struct.
pub fn schema_for<T: schemars::JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T))
        .unwrap_or_else(|_| serde_json::json!({ "type": "object" }))
}
