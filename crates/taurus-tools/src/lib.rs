//! Tools the agent can call, and the permission gate in front of them.

pub mod budget;
pub mod builtin;
pub mod capture;
pub mod checkpoint;
pub mod coerce;
pub mod delegate;
pub mod diff;
pub mod env;
pub mod jobs;
pub mod login_path;
pub mod overflow;
pub mod path_guard;
pub mod pending;
pub mod permission;
pub mod plan;
pub mod registry;
pub mod schema;
pub mod sweep;
pub mod tool;
pub mod vault;
pub mod view;

pub use budget::OutputBudget;
pub use builtin::pty::sideload_status;
pub use checkpoint::{Checkpoint, CheckpointStore, Restored, Rewind, TurnChange, TurnRecorder};
pub use delegate::{DelegateReport, Disposition, Owner, Touched};
pub use diff::{DiffHunk, DiffLine, DiffLineKind, FileDiff};
pub use env::expand_env;
pub use jobs::{BackgroundJob, JobOutput, Jobs};
pub use login_path::Outcome as LoginPath;
pub use pending::{Arrival, Pending, Ticket};
pub use permission::{
    compound_reason, AllowAll, AllowedRule, DenyAll, PermissionDecision, PermissionEngine,
    PermissionPrompt, PermissionRequest, Scope,
};
pub use plan::PlanBoard;
pub use registry::ToolRegistry;
pub use sweep::{Sweep, SweepCache};
pub use taurus_process::no_console;
pub use tool::{Effect, Tool, ToolContext, ToolError, ToolProgress, ToolResult};
pub use vault::SecretVault;
pub use view::{Answer, Asker, Question, Step, StepState, TranscriptView, Unattended};

#[cfg(test)]
mod test_support {
    use std::sync::Arc;

    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use crate::permission::{AllowAll, DenyAll, PermissionEngine};
    use crate::tool::ToolContext;

    /// A context over a fresh temp workspace that approves everything.
    pub fn test_ctx() -> (ToolContext, TempDir) {
        build(Box::new(AllowAll))
    }

    /// Same, but every permission prompt is denied.
    pub fn test_ctx_denying() -> (ToolContext, TempDir) {
        build(Box::new(DenyAll))
    }

    /// `10000` as `10,000`, the way a description written for a reader
    /// spells it.
    pub fn grouped(n: usize) -> String {
        let digits = n.to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3);
        for (i, digit) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(digit);
        }
        out
    }

    fn build(prompt: Box<dyn crate::permission::PermissionPrompt>) -> (ToolContext, TempDir) {
        let dir = TempDir::new().unwrap();
        // Canonicalize up front: on macOS the temp dir is behind /var -> /private/var,
        // and a workspace root that is itself a symlink would fail every path check.
        let root = dir.path().canonicalize().unwrap();
        // The global layer points inside the temp workspace so a test grant
        // cannot reach the real config home.
        let engine = Arc::new(PermissionEngine::new(&root, root.join(".taurus"), prompt));
        (
            ToolContext::new(root, engine, CancellationToken::new()),
            dir,
        )
    }
}
