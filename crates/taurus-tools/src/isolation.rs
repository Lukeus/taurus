//! A folder of its own for a delegate that writes.
//!
//! Two delegates editing one working tree would each write over the other, so
//! a delegate that can write runs alone unless it has somewhere else to work.
//! This is the seam for that somewhere else. The host provides it, with a git
//! worktree, because only the host knows where such things may live. The
//! delegation asks for one, works in it, and closes it, and what the delegate
//! changed is kept for somebody to apply.
//!
//! Three rules hold whatever provides it:
//!
//! - **Nothing falls back to the shared folder.** When a folder can't be
//!   opened, [`Isolation::open`] says why, and the delegate runs alone in the
//!   main folder the way a writer always has. It never runs beside another
//!   writer there.
//! - **Closing keeps the work.** [`Isolation::close`] saves what the delegate
//!   changed before it removes anything, so a folder that can't be removed
//!   costs disk, not work.
//! - **Nothing reaches the person's folder.** Applying what was kept is a
//!   separate step somebody takes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Who a folder is for, written down beside it so a leftover can be traced
/// back to the conversation and call that made it.
#[derive(Clone, Debug, Default)]
pub struct Requester {
    pub agent: String,
    pub session: Option<String>,
    pub call: Option<String>,
}

/// A folder opened for one delegation.
#[derive(Clone, Debug)]
pub struct Opened {
    /// Names this run to [`Isolation::close`], and later to Apply.
    pub run: String,
    /// Where the delegate works: the same place in the copy that the
    /// workspace is in the repository, so a path the parent would write as
    /// `src/a.rs` means the same file to the delegate.
    pub root: PathBuf,
    /// The commit it starts from, short. It holds the workspace as it stood
    /// when the delegate started, uncommitted and untracked files included.
    pub base: String,
    /// Said to the delegate and on its card, when something about the copy is
    /// worth knowing. Today, which ignored files were copied in.
    pub note: Option<String>,
}

/// What a delegate in its own folder left behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Kept {
    /// The run, which Apply and Discard name.
    pub run: String,
    /// The commit it started from, short.
    pub base: String,
    /// Workspace-relative paths it changed, added or deleted, sorted.
    pub files: Vec<String>,
    /// Paths it changed outside the workspace but inside the repository. They
    /// are kept and listed, and never applied: the workspace is all Taurus
    /// writes to.
    #[serde(default)]
    pub outside: Vec<String>,
}

/// What closing a folder came to.
#[derive(Clone, Debug)]
pub struct Closed {
    /// `None` when the delegate changed nothing, and nothing was kept.
    pub kept: Option<Kept>,
    /// Said when the folder itself couldn't be removed. The work is safe in
    /// [`Kept`] either way, and the leftover is retried on the next start.
    pub left_behind: Option<String>,
}

/// Provides delegates with folders of their own. See the module note.
#[async_trait::async_trait]
pub trait Isolation: Send + Sync {
    /// A folder for one delegation of `workspace`, or a sentence saying why
    /// there can't be one, which the delegate's card shows.
    async fn open(&self, workspace: &Path, who: &Requester) -> Result<Opened, String>;

    /// Saves what the delegation changed, and removes its folder.
    async fn close(&self, run: &str) -> Result<Closed, String>;
}
