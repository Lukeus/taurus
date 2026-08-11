//! Web search and page fetching, exposed to the agent as ordinary tools.
//!
//! Both leave the machine, so both carry [`Effect::Network`] and prompt with the
//! query or URL they would send. Neither is registered unless the user has
//! configured a search backend — see [`config`] — because a tool the model can
//! see is a tool it will try, and one that cannot work costs it a turn to find
//! that out.
//!
//! [`Effect::Network`]: taurus_tools::Effect::Network

pub mod address;
pub mod config;
pub mod fetch;
mod http;
pub mod search;

pub use config::{
    env_key, load, merge, merge_with, starter_file, Backend, BackendEntry, BackendKind, SearchFile,
};
pub use fetch::FetchUrl;
pub use search::WebSearch;

#[cfg(test)]
mod test_support {
    use std::sync::Arc;

    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use taurus_tools::{AllowAll, PermissionEngine, ToolContext};

    /// A context that approves everything, over a throwaway workspace.
    ///
    /// The web tools never touch the workspace, but they do consult the
    /// permission engine, and one pointed at the real config home would read
    /// the user's actual grants.
    pub fn allowing_ctx() -> (ToolContext, TempDir) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let engine = Arc::new(PermissionEngine::new(
            &root,
            root.join(".taurus"),
            Box::new(AllowAll),
        ));
        (
            ToolContext::new(root, engine, CancellationToken::new()),
            dir,
        )
    }
}
