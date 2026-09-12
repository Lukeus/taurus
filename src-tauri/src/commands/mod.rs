//! Tauri commands: the entire surface the frontend can reach.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::mapref::entry::Entry;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{Emitter, State};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use ts_rs::TS;

use taurus_agents::proposal::{
    save as save_agent_file, validate_proposal as validate_agent, AgentProposal,
    SaveTarget as AgentSaveTarget,
};
use taurus_agents::{AgentSummary, DESCRIPTION_LIMIT, MAX_ITERATIONS_LIMIT};
use taurus_core::{Session, UiEvent};
use taurus_mcp::ServerStatus;
use taurus_provider::{ChatRequest, Message, ModelInfo, StopReason, StreamAccumulator};
use taurus_skills::proposal::{save, SaveTarget, SkillProposal};
use taurus_skills::skill::SkillSummary;
use taurus_tools::{AllowedRule, Answer, BackgroundJob, JobOutput, PermissionDecision, Scope};

use taurus_data::{
    Dataset, Materialized as DataRun, Page as DataPage, Profile as DataProfile,
    QueryResult as DataQueryResult, Recipe,
};

use taurus_host::onscreen::OnScreen;
use taurus_host::search::{self, SearchResults};
use taurus_host::traces::{self, TraceReport};
use taurus_host::trust::TrustStatus;
use taurus_host::usage::{self, UsageReport};
use taurus_host::{
    sessions, Attachment, BackendKind, Checkpoint, Commit, CustomTheme, Document, Host, KeyStatus,
    McpServerDraft, McpServerRef, McpServerView, Note, Page, PageKind, PageRef, PageSaved, Problem,
    ProviderConfig, Repo, RepoStatus, Rewind, Saved, SessionLog, SessionMeta, Settings, Switch,
    Theme, ThemeFile, TurnChange, TurnRef,
};

use crate::state::{AppState, SessionEntry};
use crate::terminal::TerminalEvent;

/// Commands return this so the frontend gets a readable message rather than a
/// serialized Rust error.
pub type CmdResult<T> = Result<T, String>;

/// Runs synchronous filesystem work somewhere other than the runtime.
///
/// Every `#[tauri::command]` here is `async` and therefore runs on a shared
/// Tokio worker — the same pool the forwarder pumping a live turn's tokens into
/// the webview runs on. A command that reads and parses a multi-megabyte
/// transcript in-line occupies one of those workers for the whole read, and the
/// stream it stalls is not the one the user clicked on. The lower layers of the
/// tree already draw this line (see `taurus_tools::sweep`); the command layer
/// did not.
///
/// The `Err` a panicking task produces is deliberately not swallowed into a
/// plausible-looking value: a command that reports "no such conversation"
/// because its worker crashed is a bug that reads like a missing file.
async fn off_runtime<T, F>(work: F) -> CmdResult<T>
where
    F: FnOnce() -> CmdResult<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(e) => Err(format!("reading from disk failed: {e}")),
    }
}

/// The model a conversation is on, without waiting for a turn running in it.
/// See [`SessionEntry::model`].
async fn session_model(entry: &SessionEntry) -> String {
    entry.model.lock().await.clone()
}

/// Refuses a turn in a conversation that belongs to another folder.
///
/// A conversation is bound to a workspace three ways at once: its transcript is
/// filed under that folder, its checkpoints are keyed by it, and every path it
/// has ever named describes that tree. A turn sent from somewhere else runs
/// against the open workspace — its files, its permission rules, its
/// checkpoints — and appends to a transcript filed under the other one, leaving
/// a single conversation split across two projects with neither half complete.
///
/// The window closes a conversation when it changes folders, so this should
/// never be reached. It is here to make that a rule rather than a habit: the
/// frontend is one of several things that can call a command, and the one
/// consequence of getting it wrong is silent and on disk.
///
/// The message names the folder rather than saying "wrong workspace", because
/// the way out is to go back to it — and with two checkouts of the same project
/// open, the name alone would not say which.
fn check_workspace(session: &std::path::Path, open: &std::path::Path) -> CmdResult<()> {
    if session == open {
        return Ok(());
    }
    Err(format!(
        "This conversation belongs to {}. Open that folder to continue it, or \
         start a new conversation here.",
        session.display()
    ))
}

/// The workspace a conversation belongs to, which is not always the one open.
///
/// Live conversations answer from memory; a saved one from its transcript's
/// header, which is cheap — [`sessions::workspace_of`] reads the top of the
/// file, not the file. A conversation that is neither is one there is nothing
/// to read for, and the open workspace is as good an answer as any for the
/// "no such session" that follows.
async fn session_workspace(state: &AppState, session_id: &str) -> PathBuf {
    if let Ok(entry) = state.session(session_id) {
        return entry.workspace.clone();
    }
    // Off the runtime, cheap as it is: finding the transcript lists every
    // workspace's directory before the header is read.
    let id = session_id.to_string();
    let saved = off_runtime(move || Ok(sessions::workspace_of(&id)))
        .await
        .ok()
        .flatten();
    match saved {
        Some(workspace) => workspace,
        None => state.host.workspace().await,
    }
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct AppStatus {
    pub workspace: String,
    pub providers: Vec<ProviderConfig>,
    pub settings: Settings,
    pub skill_count: usize,
    /// How many notes earlier conversations in this workspace left behind, for
    /// the badge on the rail. Zero draws nothing rather than a `0`.
    pub note_count: usize,
    /// The roster size, so the rail can carry it beside the skill count. Never
    /// zero: two agents ship with the harness.
    pub agent_count: usize,
    /// How many datasets are loaded here.
    ///
    /// The Data pane does not exist until this is non-zero, which is the whole
    /// of how that surface stays out of the way of everybody who is not using
    /// it. A count rather than the list: the tab only needs to know whether
    /// there is anything behind it, and the pane fetches the rest when it
    /// opens.
    pub dataset_count: usize,
    /// Everything that failed to load, each tagged with where it came from so
    /// the UI can show it on the screen that can fix it. Previously this was an
    /// untagged list called `skill_problems`, and a malformed `providers.json`
    /// was reported under a list of skills.
    pub problems: Vec<Problem>,
    pub tool_names: Vec<String>,
    pub mcp_servers: Vec<ServerStatus>,
    /// The custom theme in force, already read and checked, or `None` for the
    /// built-in palette.
    ///
    /// The resolved theme rather than its id, because the window has to be
    /// able to paint it on the first frame it has settings for — a rail that
    /// renders in the shipped cyan and then repaints in somebody's brand a
    /// round trip later is the flash this avoids. Only the *active* one is
    /// here: a theme carries its logo inlined, and the picker's full list is
    /// fetched when the picker opens rather than pushed with every status.
    #[ts(optional)]
    pub theme: Option<CustomTheme>,
    /// The branch checked out in this workspace, when there is one.
    ///
    /// A snapshot taken with the rest of the status, which is enough for what
    /// the rail does with it — mark the conversations that were started
    /// somewhere else. Anything about to *write* asks
    /// [`repo_status`] instead, which reads afresh: a branch switched in a
    /// terminal beside this window must not be answered from a cache at the
    /// moment before someone commits.
    #[ts(optional)]
    pub branch: Option<String>,
}

/// Everything the shell shows about the app as a whole, read now.
pub async fn status_of(state: &AppState) -> AppStatus {
    AppStatus {
        // Plain, not verbatim: the rail splits this into components to show
        // where the window is pointed. See `taurus_tools::path_guard::plain`.
        workspace: taurus_tools::path_guard::plain(&state.host.workspace().await)
            .display()
            .to_string(),
        providers: state.host.providers().await,
        settings: state.host.settings().await,
        skill_count: state.host.skill_count().await,
        note_count: state.host.notes().await.len(),
        agent_count: state.host.agents().await.len(),
        dataset_count: state.host.datasets().await.len(),
        problems: state.host.problems().await,
        tool_names: state.host.tool_names().await,
        mcp_servers: state.host.mcp_statuses().await,
        theme: state.host.active_theme().await,
        branch: state.host.branch().await,
    }
}

/// Pushes the current status to the window.
///
/// Called at the end of anything that can move a number the shell is showing —
/// a reload, a workspace switch, a settings write, a turn that left a note
/// behind. The frontend does not ask for status again after startup, so a
/// change that forgets to come through here is one the user sees the old value
/// of until something unrelated happens.
///
/// Never fails a command. A window that has gone away is not a reason to refuse
/// the work that was done for it.
pub async fn emit_status(state: &AppState) {
    let status = status_of(state).await;
    if let Err(e) = state.app.emit(crate::bridge::EVENT_STATUS, &status) {
        tracing::warn!(error = %e, "could not push the status to the window");
    }
}

/// Every file one conversation has changed, after something cut the set back.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct ChangedFiles {
    pub session: String,
    /// Workspace-relative, deduplicated across every turn still in the log.
    pub files: Vec<String>,
}

/// Pushes that set to the window, read from the checkpoint log.
///
/// Only for the paths that *shrink* it. A turn reports what it changes on its
/// own event stream as it changes them, which is both cheaper and in order with
/// everything else the turn is saying.
pub async fn emit_changed(state: &AppState, session_id: &str) {
    let workspace = session_workspace(state, session_id).await;
    // Off the runtime, the same read `list_checkpoints` makes and for the same
    // reason: the whole log is parsed to learn the names in it.
    let store = state.host.checkpoints_for(&workspace);
    let id = session_id.to_string();
    let Ok(turns) = off_runtime(move || store.turns(&id)).await else {
        return;
    };

    let mut files: Vec<String> = turns
        .into_iter()
        .flat_map(|turn| turn.files)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    files.dedup();

    let payload = ChangedFiles {
        session: session_id.to_string(),
        files,
    };
    if let Err(e) = state.app.emit(crate::bridge::EVENT_CHANGED, &payload) {
        tracing::warn!(error = %e, "could not push the changed files to the window");
    }
}

/// Pushes one conversation's listing entry to the window.
///
/// The frontend merges by id, so this is what a turn ending, a rename, or a
/// transcript first reaching disk costs: one file read instead of a scan of
/// every transcript in the workspace.
pub async fn emit_session(state: &AppState, session_id: &str) {
    // Off the runtime: finding the transcript lists every workspace's
    // directory, and this follows every turn.
    let id = session_id.to_string();
    let Ok(Some(meta)) = off_runtime(move || Ok(sessions::meta(&id))).await else {
        return;
    };
    if let Err(e) = state.app.emit(crate::bridge::EVENT_SESSION, &meta) {
        tracing::warn!(error = %e, "could not push a conversation to the window");
    }
}

mod changes;
mod data;
mod library;
mod mcp;
mod notes;
mod providers;
mod session;
mod settings;
mod terminal;
mod web_search;
mod workspace;

pub use changes::*;
pub use data::*;
pub use library::*;
pub use mcp::*;
pub use notes::*;
pub use providers::*;
pub use session::*;
pub use settings::*;
pub use terminal::*;
pub use web_search::*;
pub use workspace::*;

#[cfg(test)]
mod tests;
