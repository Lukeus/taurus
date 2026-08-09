//! Tools the agent can call, and the permission gate in front of them.

pub mod builtin;
pub mod coerce;
pub mod path_guard;
pub mod permission;
pub mod registry;
pub mod tool;

pub use permission::{
    AllowAll, DenyAll, PermissionDecision, PermissionEngine, PermissionPrompt, PermissionRequest,
};
pub use registry::ToolRegistry;
pub use tool::{Effect, Tool, ToolContext, ToolError, ToolResult};

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

    fn build(prompt: Box<dyn crate::permission::PermissionPrompt>) -> (ToolContext, TempDir) {
        let dir = TempDir::new().unwrap();
        // Canonicalize up front: on macOS the temp dir is behind /var -> /private/var,
        // and a workspace root that is itself a symlink would fail every path check.
        let root = dir.path().canonicalize().unwrap();
        let engine = Arc::new(PermissionEngine::new(&root, prompt));
        (
            ToolContext::new(root, engine, CancellationToken::new()),
            dir,
        )
    }
}
