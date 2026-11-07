//! What delegates kept in copies of the workspace: applying it, discarding it,
//! and listing what's still waiting. See [`crate::worktree`].

use super::*;

impl Host {
    /// Applies what delegate run `run` kept, for a person pressing Apply.
    ///
    /// Recorded into the checkpoint of the conversation's latest turn (one is
    /// opened under that turn's id when it changed nothing itself), so it's
    /// undone by rewinding that turn and a fork still finds one checkpoint per
    /// transcript turn. A model applying from inside a turn
    /// goes through `apply_delegate` instead, which records into the turn it's
    /// in.
    pub async fn apply_delegate(
        &self,
        session_id: &str,
        run: &str,
        dry_run: bool,
    ) -> Result<taurus_tools::ApplyPlan, String> {
        let record = self.worktrees.record(run)?;
        let store = self.checkpoints_for(&record.workspace);
        // A conversation whose files are set aside has another branch's files
        // on disk, and applying there would land this one's work in that one.
        crate::fork::ensure_on_disk(&store, session_id)?;
        use taurus_tools::Isolation;
        if dry_run {
            return self
                .worktrees
                .apply(run, Some(session_id), None, true)
                .await;
        }
        let prompt = format!("Applied what {} kept (run {run})", record.agent);
        let recorder = match crate::sessions::last_turn_id(session_id) {
            Some(turn) => store.begin_turn_under(session_id, &record.workspace, &prompt, &turn),
            // No transcript turn to file it under (its transcript is missing
            // or unreadable). Recorded unnamed it still rewinds, and a fork
            // says why it can't; refused, the work couldn't be applied at all.
            None => store.begin_turn(session_id, &record.workspace, &prompt),
        };
        self.worktrees
            .apply(run, Some(session_id), Some(&recorder), false)
            .await
    }

    /// Lets go of what delegate run `run` kept, without applying it.
    pub async fn discard_delegate(&self, session_id: &str, run: &str) -> Result<(), String> {
        self.worktrees.discard(run, Some(session_id)).await
    }

    /// Every delegate run with work still kept or a folder left behind, newest
    /// first, so nothing piles up where nobody can see it.
    pub fn delegate_runs(&self) -> Vec<crate::worktree::RunSummary> {
        self.worktrees.list()
    }
}
