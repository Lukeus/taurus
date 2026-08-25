//! Semantic search over the workspace, embedded locally.
//!
//! An embedding index is normally a cloud dependency: a service to send your
//! code to, a bill, and a vector database to run. For a local-first harness it
//! is none of those. The machine already has a model server on it — the one
//! answering the conversation — so an index is one more endpoint on the same
//! server, and the vectors are a file in the config home.
//!
//! That is the whole argument for building it here rather than reaching for
//! something. It also pairs with the constraint everything else in this
//! harness is shaped around: an 8k context cannot afford three wrong
//! `read_file` calls, and the only way to make retrieval cheap is to make it
//! accurate.
//!
//! # The parts
//!
//! - [`chunk`] cuts files into overlapping line windows.
//! - [`build`] walks the workspace and embeds what changed.
//! - [`inflight`] keeps the three things that can start a refresh from all
//!   starting one at once.
//! - [`store`] holds the vectors and searches them.
//! - [`tool`] is `search_code`, which the model calls.
//!
//! # What it is not
//!
//! There is no approximate-nearest-neighbour structure and no background
//! indexer. Both are the right call at this size, and [`store`] and [`tool`]
//! say why where they would have gone.

pub mod build;
pub mod chunk;
pub mod inflight;
pub mod store;
pub mod tool;

pub use build::{refresh, IndexProgress, Refreshed};
pub use chunk::Chunk;
pub use inflight::Indexing;
pub use store::{rerank, search, Entry, Hit, Index, Ranking};
pub use tool::{SearchCode, SEARCH_CODE_TOOL};

use std::path::Path;

/// Where one workspace's index lives.
///
/// Beside the transcripts and checkpoints, keyed the same way, for the same
/// reason: it holds the contents of files in the project, so keeping it in the
/// project would commit it.
pub fn index_dir(home: &Path, workspace_key: &str) -> std::path::PathBuf {
    home.join("index").join(workspace_key)
}

#[cfg(test)]
mod test_support {
    use std::sync::Arc;

    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use taurus_tools::{AllowAll, PermissionEngine, ToolContext};

    /// A context that approves everything, over a throwaway workspace.
    ///
    /// The root is canonicalized because that is what a real call receives —
    /// on macOS the temp directory lives behind `/var -> /private/var`, and
    /// skipping this would index absolute paths and quietly stop testing the
    /// relative ones. The permission layer is pointed inside the temp tree so
    /// no test can read the user's actual grants.
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
