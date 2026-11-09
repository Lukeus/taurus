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
    /// A folder outside the copy for the run's own records (its undo log),
    /// removed when the run closes.
    pub scratch: PathBuf,
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

/// What applying a run would do, or did, to one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ApplyAction {
    /// The workspace's copy hadn't changed since the delegate started, so it
    /// takes the delegate's.
    Take,
    /// Both changed it, and the two merged without a conflict.
    Merge,
    /// The delegate removed it, and the workspace's copy hadn't changed.
    Delete,
    /// The workspace already has what the delegate left.
    Same,
    /// It can't be applied. [`ApplyFile::why`] says why.
    Conflict,
}

/// One file in an [`ApplyPlan`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApplyFile {
    /// Workspace-relative.
    pub path: String,
    pub action: ApplyAction,
    /// Set for a conflict.
    #[ts(optional)]
    pub why: Option<String>,
}

/// What applying a run comes to, file by file. Nothing is written unless
/// every file can be: one conflict refuses the lot, so a workspace is never
/// left with half a delegate's change in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApplyPlan {
    pub run: String,
    pub files: Vec<ApplyFile>,
    /// Changed outside the workspace. Never applied.
    pub outside: Vec<String>,
    /// Whether it was written. False for a look ahead, and for a refusal.
    pub applied: bool,
}

impl ApplyPlan {
    pub fn conflicts(&self) -> impl Iterator<Item = &ApplyFile> {
        self.files
            .iter()
            .filter(|f| f.action == ApplyAction::Conflict)
    }

    /// The plan as a model or a CLI reads it.
    pub fn describe(&self) -> String {
        let conflicts: Vec<String> = self
            .conflicts()
            .map(|f| {
                format!(
                    "- {}: {}",
                    f.path,
                    f.why.as_deref().unwrap_or("it can't be applied")
                )
            })
            .collect();
        if !conflicts.is_empty() {
            return format!(
                "Nothing was applied from run {}, because {} of its files can't be:\n{}\nThe rest \
                 are still kept. Make those changes by hand, or ask for the work again from the \
                 workspace as it is now.",
                self.run,
                conflicts.len(),
                conflicts.join("\n")
            );
        }
        let count = |action| self.files.iter().filter(|f| f.action == action).count();
        let verb = if self.applied {
            "Applied"
        } else {
            "Would apply"
        };
        let mut text = format!(
            "{verb} run {}: {} taken, {} merged, {} deleted, {} already the same.",
            self.run,
            count(ApplyAction::Take),
            count(ApplyAction::Merge),
            count(ApplyAction::Delete),
            count(ApplyAction::Same)
        );
        if self.applied {
            text.push_str(
                " It's one change in the undo history, so rewinding this turn undoes it.",
            );
        }
        if !self.outside.is_empty() {
            text.push_str(&format!(
                " Not applied, because they're outside the workspace: {}.",
                self.outside.join(", ")
            ));
        }
        text
    }
}

/// Provides delegates with folders of their own. See the module note.
#[async_trait::async_trait]
pub trait Isolation: Send + Sync {
    /// A folder for one delegation of `workspace`, or a sentence saying why
    /// there can't be one, which the delegate's card shows.
    async fn open(&self, workspace: &Path, who: &Requester) -> Result<Opened, String>;

    /// Saves what the delegation changed, and removes its folder.
    async fn close(&self, run: &str) -> Result<Closed, String>;

    /// Applies what `run` kept to its workspace, or with `dry_run` says what
    /// that would do. Each file is captured into `recorder` before it's
    /// written, so the change is undoable. `session` has to be the
    /// conversation the run was made in.
    async fn apply(
        &self,
        run: &str,
        session: Option<&str>,
        recorder: Option<&crate::checkpoint::TurnRecorder>,
        dry_run: bool,
    ) -> Result<ApplyPlan, String>;

    /// What `run` kept, as a unified diff against the commit it started from,
    /// with a `--stat` summary first. Read-only. `session` has to be the
    /// conversation the run was made in, as for [`Self::apply`].
    ///
    /// Defaulted, because a provider that keeps nothing to diff has nothing to
    /// say here, and saying so is the right answer.
    async fn diff(&self, run: &str, session: Option<&str>) -> Result<String, String> {
        let _ = session;
        Err(format!("run {run}'s changes can't be shown here"))
    }
}
