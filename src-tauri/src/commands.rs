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
use taurus_provider::{ChatRequest, Message, ModelInfo, StreamAccumulator};
use taurus_skills::proposal::{save, SaveTarget, SkillProposal};
use taurus_skills::skill::SkillSummary;
use taurus_tools::{AllowedRule, Answer, PermissionDecision, Scope};

use taurus_data::{
    Dataset, Materialized as DataRun, Page as DataPage, Profile as DataProfile,
    QueryResult as DataQueryResult, Recipe,
};

use taurus_host::onscreen::OnScreen;
use taurus_host::trust::TrustStatus;
use taurus_host::{
    sessions, Attachment, BackendKind, Checkpoint, Commit, Host, KeyStatus, McpServerDraft,
    McpServerRef, McpServerView, Note, Problem, ProviderConfig, Repo, RepoStatus, Rewind,
    SessionLog, SessionMeta, Settings, Switch, Theme, TurnChange, TurnRef,
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

async fn session_model(entry: &Arc<SessionEntry>) -> String {
    entry.session.lock().await.model.clone()
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
    match sessions::workspace_of(session_id) {
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
        workspace: state.host.workspace().await.display().to_string(),
        providers: state.host.providers().await,
        settings: state.host.settings().await,
        skill_count: state.host.skill_count().await,
        note_count: state.host.notes().await.len(),
        agent_count: state.host.agents().await.len(),
        dataset_count: state.host.datasets().await.len(),
        problems: state.host.problems().await,
        tool_names: state.host.tool_names().await,
        mcp_servers: state.host.mcp_statuses().await,
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
    let Ok(turns) = state.host.checkpoints_for(&workspace).turns(session_id) else {
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
    let Some(meta) = sessions::meta(session_id) else {
        return;
    };
    if let Err(e) = state.app.emit(crate::bridge::EVENT_SESSION, &meta) {
        tracing::warn!(error = %e, "could not push a conversation to the window");
    }
}

#[tauri::command]
pub async fn get_status(state: State<'_, Arc<AppState>>) -> CmdResult<AppStatus> {
    // The frontend asks for this once, on mount, and keeps what it gets. That
    // races the startup reload, and losing the race meant a permanent `0`
    // beside a drawer full of skills — see [`AppState::loaded`]. Every later
    // change arrives on `EVENT_STATUS` rather than by being asked for again.
    state.loaded().await;
    Ok(status_of(&state).await)
}

#[tauri::command]
pub async fn set_workspace(state: State<'_, Arc<AppState>>, path: String) -> CmdResult<String> {
    let resolved = state.host.set_workspace(&PathBuf::from(path)).await?;
    info!(workspace = %resolved.display(), "workspace changed");
    // Everything the shell shows about the app belongs to the folder, so all of
    // it has just changed at once.
    emit_status(&state).await;
    Ok(resolved.display().to_string())
}

/// Whether this workspace's own config is being read, and what it holds.
///
/// Polled by the app rather than pushed, because the answer changes when a file
/// appears in a directory nobody is watching — a `git pull` that adds
/// `.taurus/mcp.json` is exactly the case the gate exists for, and it arrives
/// with no event attached to it. See [`taurus_host::trust`].
#[tauri::command]
pub async fn workspace_trust(state: State<'_, Arc<AppState>>) -> CmdResult<TrustStatus> {
    Ok(state.host.trust_status().await)
}

#[tauri::command]
pub async fn trust_workspace(state: State<'_, Arc<AppState>>) -> CmdResult<TrustStatus> {
    state.host.trust_workspace().await?;
    let status = state.host.trust_status().await;
    info!(workspace = %status.workspace, "workspace trusted");
    // Saying yes is what loads this project's skills, agents and servers; the
    // counts on the rail move with it.
    emit_status(&state).await;
    Ok(status)
}

#[tauri::command]
pub async fn revoke_workspace_trust(state: State<'_, Arc<AppState>>) -> CmdResult<TrustStatus> {
    state.host.revoke_trust().await?;
    let status = state.host.trust_status().await;
    info!(workspace = %status.workspace, "workspace trust revoked");
    emit_status(&state).await;
    Ok(status)
}

#[tauri::command]
pub async fn list_models(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
) -> CmdResult<Vec<ModelInfo>> {
    let provider = state.host.provider(&provider_id).await?;
    provider.models().await.map_err(|e| e.to_string())
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct CreatedSession {
    pub id: String,
    pub model: String,
    pub provider_id: String,
    /// False when the model has no native tool support and the prompted
    /// fallback will be used. Shown in the UI because it changes reliability.
    pub native_tools: bool,
    /// Whether this model reads images. Decides whether the composer offers to
    /// attach one at all — an attach button on a model that cannot see is an
    /// invitation to a refusal.
    pub vision: bool,
    pub context_length: u32,
}

#[tauri::command]
pub async fn create_session(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    model: String,
) -> CmdResult<CreatedSession> {
    let provider = state.host.provider(&provider_id).await?;
    let capabilities = provider
        .capabilities(&model)
        .await
        .map_err(|e| e.to_string())?;

    let session = Session::new(&model);
    // Stamped at creation rather than looked up later: which branch the work
    // was done on is only knowable while it is happening. The same is true of
    // the workspace, which is why the entry carries it from here on.
    let workspace = state.host.workspace().await;
    let branch = state.host.branch().await;
    let log = SessionLog::create(&session, &workspace, branch);
    let id = session.id.clone();
    state.sessions.insert(
        id.clone(),
        Arc::new(SessionEntry {
            session: Arc::new(Mutex::new(session)),
            provider_id: Mutex::new(provider_id.clone()),
            workspace,
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            log: Arc::new(Mutex::new(log)),
            // A conversation starts where it starts; a switch is what puts
            // anything in here.
            switches: Mutex::new(Vec::new()),
        }),
    );

    state.host.remember_session(&provider_id, &model).await;

    info!(session = %id, %model, native_tools = capabilities.native_tools, "session created");
    Ok(CreatedSession {
        id,
        model,
        provider_id,
        native_tools: capabilities.native_tools,
        vision: capabilities.vision,
        context_length: capabilities.context_length,
    })
}

/// Saved conversations, newest first — this workspace's, or every one.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, Arc<AppState>>,
    all: bool,
) -> CmdResult<Vec<SessionMeta>> {
    // Every transcript in the workspace is opened and partly parsed to build
    // this, and with `all` every transcript on the machine is — so it is one of
    // the two reads that most needs to be off the runtime.
    let workspace = state.host.workspace().await;
    off_runtime(move || Ok(sessions::list(if all { None } else { Some(&workspace) }))).await
}

/// What a resumed conversation needs to be redrawn and continued.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct ResumedSession {
    pub id: String,
    pub model: String,
    pub provider_id: String,
    pub native_tools: bool,
    /// Whether this model reads images. See [`CreatedSession::vision`].
    pub vision: bool,
    pub context_length: u32,
    /// The whole transcript, for the frontend to rebuild the view from.
    pub messages: Vec<Message>,
    /// Where this conversation changed model, each positioned by how much of
    /// the transcript came before it — so a reopened conversation shows the
    /// change where it happened rather than only what it ended on.
    pub switches: Vec<Switch>,
}

/// The conversation one delegate had, for reading.
///
/// Deliberately not a resume. A delegate's transcript is a record of work that
/// happened inside a turn, not a conversation to be carried on: it has no
/// provider bound to it, no workspace of its own, and continuing it would mean
/// a second live session nobody asked for. The frontend gets the messages and
/// draws them read-only.
///
/// Scoped to the parent, because that is what a delegate's id is unique
/// *within*. Both ids are validated against the sessions tree before either
/// touches the filesystem — see `taurus_host::sessions`.
#[tauri::command]
pub async fn read_subagent_transcript(
    session_id: String,
    subagent_id: String,
) -> CmdResult<Vec<Message>> {
    off_runtime(move || {
        sessions::load_subagent(&session_id, &subagent_id).map(|loaded| loaded.session.messages)
    })
    .await
}

/// Reopens a saved conversation as a live session.
///
/// Already-open sessions are returned as they stand rather than reloaded: the
/// in-memory one is the newer of the two mid-turn, and replacing it would drop
/// whatever the running turn has produced.
#[tauri::command]
pub async fn resume_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    provider_id: Option<String>,
) -> CmdResult<ResumedSession> {
    // `loaded` is carried alongside so the log below can be opened on the
    // transcript this was actually read from — see `SessionLog::resume`.
    let (session, provider_id, switches, loaded) = match state.sessions.get(&session_id) {
        Some(open) => {
            let entry = open.clone();
            let provider_id = entry.provider_id.lock().await.clone();
            let switches = entry.switches.lock().await.clone();
            // Never waited for. A turn holds this lock for its whole run, which
            // is minutes for a long one, and awaiting it here hung the window
            // on the click that reopens the conversation that is *already*
            // streaming — the one case where the frontend does not close the
            // previous session first, so the one case that could reach it.
            //
            // `delete` and `rewind` meet the same lock and refuse; this does
            // not, because reopening a conversation has to work. What it falls
            // back to is what a cold open would have read: the transcript on
            // disk, complete to the end of the last round. The turn's own
            // stream carries on appending from there.
            let live = entry.session.try_lock().ok().map(|s| s.clone());
            match live {
                Some(session) => (session, provider_id, switches, None),
                None => {
                    let id = session_id.clone();
                    let loaded = off_runtime(move || sessions::load(&id)).await?;
                    // Deliberately not `Some(loaded)`: that is what opens a log
                    // and installs a session entry, and this conversation
                    // already has both — with a turn running through them.
                    (loaded.session, provider_id, switches, None)
                }
            }
        }
        None => {
            // The whole `.jsonl` — file contents, shell output, MCP results
            // and all — parsed a line at a time. Several megabytes is ordinary
            // for a long coding conversation, and this is the click that used
            // to stutter an unrelated live stream.
            let id = session_id.clone();
            let loaded = off_runtime(move || sessions::load(&id)).await?;
            // Whichever provider the caller is on; failing that the one this
            // conversation was last worked on, which is known only for one that
            // has moved at least once; failing that whatever the host resolves.
            // A header records the model but deliberately not the backend that
            // served it, and that backend may not even be configured now.
            let (resolved, _) = state
                .host
                .resolve_model(
                    provider_id.as_deref().or(loaded.provider.as_deref()),
                    Some(&loaded.session.model),
                )
                .await?;
            let switches = loaded.switches.clone();
            (loaded.session.clone(), resolved, switches, Some(loaded))
        }
    };

    let provider = state.host.provider(&provider_id).await?;
    let capabilities = provider
        .capabilities(&session.model)
        .await
        .map_err(|e| e.to_string())?;

    let resumed = ResumedSession {
        id: session.id.clone(),
        model: session.model.clone(),
        provider_id: provider_id.clone(),
        native_tools: capabilities.native_tools,
        vision: capabilities.vision,
        context_length: capabilities.context_length,
        messages: session.messages.clone(),
        switches: switches.clone(),
    };

    // Only if it is still absent. The awaits above are where a second resume of
    // the same conversation would arrive, and the entry it had already made is
    // the newer of the two — an insert here would drop the log it is appending
    // through and its in-flight turn's cancellation token with it.
    if let Some(loaded) = loaded {
        let log = SessionLog::resume(&loaded);
        if let Entry::Vacant(slot) = state.sessions.entry(session_id.clone()) {
            slot.insert(Arc::new(SessionEntry {
                session: Arc::new(Mutex::new(session)),
                provider_id: Mutex::new(provider_id),
                // The conversation's own folder, out of its header — not the
                // one open now. They are the same in the ordinary case and
                // must not be assumed to be.
                workspace: loaded.workspace,
                cancel: Arc::new(Mutex::new(CancellationToken::new())),
                log: Arc::new(Mutex::new(log)),
                switches: Mutex::new(switches),
            }));
            info!(session = %session_id, "session resumed");
        }
    }

    Ok(resumed)
}

/// Runs one turn, streaming events to `on_event`.
///
/// A `Channel` rather than a global event: delivery is ordered and scoped to
/// this call, so two sessions streaming at once cannot interleave in the UI.
#[tauri::command]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    text: String,
    images: Option<Vec<Attachment>>,
    // What the Data pane was showing, when the message was sent from it. It
    // reaches the model and not the transcript — see `taurus_host::onscreen`,
    // which explains the split and what it costs.
    on_screen: Option<OnScreen>,
    on_event: Channel<UiEvent>,
) -> CmdResult<()> {
    let entry = state.session(&session_id)?;

    check_workspace(&entry.workspace, &state.host.workspace().await)?;

    // Read once, here, so a turn is sent to the backend this conversation was
    // on when it began even if somebody moves it while the answer streams.
    let provider_id = entry.provider_id.lock().await.clone();
    let provider = state.host.provider(&provider_id).await?;

    // `/name args` becomes the skill's procedure — or the instruction to
    // delegate to that sub-agent — before the model sees it. The user's own
    // line stays what the transcript shows and what names the turn: an
    // expansion is how the request is carried out, not what was asked.
    let prompt = match state.host.expand_command(&text).await {
        Some(Ok(invocation)) => {
            info!(
                session = %session_id,
                command = %invocation.name,
                kind = ?invocation.kind,
                "ran a command",
            );
            invocation.prompt
        }
        // Returned rather than sent. A mistyped command is a message to the
        // user, and passing it to the model would answer a question about a
        // skill instead of running one.
        Some(Err(e)) => return Err(e.to_string()),
        None => text.clone(),
    };
    // After the command expansion rather than before it: a `/skill` invocation
    // is still being sent from a pane, and the procedure it expands to wants
    // the same subject the user had on screen.
    let prompt = taurus_host::onscreen::with_context(&prompt, on_screen.as_ref());

    // Checked before anything is started, and against this model rather than
    // this provider: on Ollama one model on a server reads images and the next
    // one does not. Refusing here costs the user a retry with the same text;
    // letting it through costs a round trip and comes back as a wire error
    // naming a field in the request body.
    let model = session_model(&entry).await;
    let images = images.unwrap_or_default();
    let blocks = if images.is_empty() {
        Vec::new()
    } else {
        let capabilities = provider
            .capabilities(&model)
            .await
            .map_err(|e| e.to_string())?;
        taurus_host::attach::to_blocks(&images, &capabilities)?
    };

    // A fresh token per turn: reusing a canceled one would abort the next turn
    // before it started.
    let cancel = CancellationToken::new();
    *entry.cancel.lock().await = cancel.clone();

    let agent = state
        .host
        .build_agent(
            provider,
            &model,
            cancel,
            TurnRef {
                session_id: &session_id,
                prompt: &text,
            },
        )
        .await
        // Persistence moves into the loop, which records once per tool round
        // trip and once when the turn ends however it ends. Two things follow.
        // The question reaches disk before the model is asked it, so a turn
        // killed half way leaves what was asked rather than nothing. And the
        // conversation becomes listable while it is being answered — with its
        // title — where before it appeared in the rail only once the turn was
        // over, which for a long turn is minutes of the app disagreeing with
        // itself about which conversations exist.
        .with_recorder(Arc::new(crate::bridge::UiSessionLog::new(
            state.app.clone(),
            entry.log.clone(),
            session_id.clone(),
        )));

    // Bridge the loop's mpsc channel to the IPC channel.
    let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
    let forwarder = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if on_event.send(event).is_err() {
                break;
            }
        }
    });

    // Images first, then the text. A model answers "what is wrong with this?"
    // better having already seen the thing, and every adapter here preserves
    // block order.
    let message = if blocks.is_empty() {
        Message::user(prompt)
    } else {
        let mut content = blocks;
        content.push(taurus_provider::ContentBlock::text(prompt));
        Message::new(taurus_provider::Role::User, content)
    };

    let mut session = entry.session.lock().await;
    // The transcript is written from inside this call, by the recorder attached
    // above — once per round and once at the end, whatever the outcome. An
    // interrupted turn still produced the messages that led there, and they are
    // already on disk in the order they happened.
    let outcome = agent.run_turn(&mut session, message, tx).await;
    drop(session);
    let _ = forwarder.await;

    // The conversation's listing entry has moved: its timestamp, and its title
    // if this was its first turn. The status has too — a turn can leave a note
    // behind, and can be the thing that moved the branch.
    emit_session(&state, &session_id).await;
    emit_status(&state).await;

    match outcome {
        Ok(outcome) => {
            info!(
                session = %session_id,
                iterations = outcome.iterations,
                "turn finished"
            );
            Ok(())
        }
        Err(e) => {
            error!(session = %session_id, error = %e, "turn failed");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn cancel_session(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    state.session(&session_id)?.cancel.lock().await.cancel();
    info!(session = %session_id, "cancel requested");
    Ok(())
}

#[tauri::command]
pub async fn close_session(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    if let Some((_, entry)) = state.sessions.remove(&session_id) {
        entry.cancel.lock().await.cancel();
    }
    // The board goes with the conversation it belongs to. `delete_session` and
    // `rewind_to` already did this; closing did not, and closing is the one
    // that happens on every switch between conversations — so the map grew by
    // one for each and never shrank. Nothing can reach a board whose session
    // entry is gone, which is what made it a leak rather than a cache.
    state.host.forget_plan(&session_id).await;
    Ok(())
}

/// Deletes a saved conversation: the transcript, and the checkpoints that made
/// its turns undoable.
///
/// Both halves go, because half of the record is worse than none — a checkpoint
/// log outliving its conversation is a copy of the user's files under an id
/// nothing can reach. Nothing in the workspace itself is touched: this forgets
/// what was said and how to undo it, not the work.
#[tauri::command]
pub async fn delete_session(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    // The same rule `rewind_to` applies, for the same reason: a turn holds this
    // lock for its whole run, and deleting underneath one would leave it
    // appending to a file that is no longer anywhere.
    if let Ok(entry) = state.session(&session_id) {
        if entry.session.try_lock().is_err() {
            return Err("this conversation is mid-turn; stop it before deleting".into());
        }
    }

    // Dropped from memory before the file goes, not after. An open session's log
    // recreates the transcript on its next write, so the other order can leave a
    // deleted conversation back on disk.
    if let Some((_, entry)) = state.sessions.remove(&session_id) {
        entry.cancel.lock().await.cancel();
    }

    sessions::delete(&session_id)?;
    state.host.checkpoints().await.forget(&session_id)?;
    state.host.forget_plan(&session_id).await;
    info!(session = %session_id, "session deleted");
    Ok(())
}

/// Moves a conversation to another model, or another backend, keeping
/// everything said in it.
///
/// The alternative — which is what both pickers used to do — is a new
/// conversation, and that is a poor trade for a question you wanted a second
/// opinion on. Nothing about the history is provider-shaped: it is stored as
/// blocks, and each adapter renders those into its own wire format on the way
/// out, drops the reasoning it cannot replay, and rewrites tool calls as text
/// for a model with no native tool support. So carrying a conversation across
/// is a matter of saying so, not of translating it.
///
/// What does change is the model's capabilities, which is why this answers with
/// them. A smaller context window compacts on the next turn, because the budget
/// is recomputed per turn from whatever model the session is on. A model that
/// cannot read images is sent the conversation with its pictures replaced by a
/// line saying one was there — see `taurus_core`'s `without_images`; the images
/// stay in the session and come back if the conversation moves to a model that
/// can see.
///
/// Reuses [`CreatedSession`] rather than declaring a near-copy: it is not the
/// creating that the shape describes, it is what the frontend has to know about
/// the live conversation, and after this call all of that has moved.
#[tauri::command]
pub async fn switch_model(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    provider_id: String,
    model: String,
) -> CmdResult<CreatedSession> {
    let entry = state.session(&session_id)?;

    // Asked before the lock is taken, and it is what makes this fail cleanly: a
    // model the backend will not serve leaves the conversation where it was
    // rather than moved to something that cannot answer it.
    let provider = state.host.provider(&provider_id).await?;
    let capabilities = provider
        .capabilities(&model)
        .await
        .map_err(|e| e.to_string())?;

    {
        // The same rule a rewind and a delete follow, and for a sharper reason
        // than either: a turn reads the model out of the session on every
        // attempt, so moving it underneath one would send half an answer to one
        // backend and half to another.
        let Ok(mut session) = entry.session.try_lock() else {
            return Err("this conversation is mid-turn; stop it before changing model".into());
        };
        session.model = model.clone();
    }
    *entry.provider_id.lock().await = provider_id.clone();

    // Written down, so reopening the conversation continues it here rather than
    // on the model in its header. Nothing is appended for a conversation with
    // no transcript yet — the first turn writes a header naming this model
    // instead. See `SessionLog::record_model`.
    if entry.log.lock().await.record_model(&provider_id, &model) {
        let session = entry.session.lock().await;
        entry.switches.lock().await.push(Switch {
            after: session.messages.len(),
            provider: provider_id.clone(),
            model: model.clone(),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
        });
    }

    state.host.remember_session(&provider_id, &model).await;
    info!(session = %session_id, %provider_id, %model, "conversation moved to another model");
    emit_status(&state).await;

    Ok(CreatedSession {
        id: session_id,
        model,
        provider_id,
        native_tools: capabilities.native_tools,
        vision: capabilities.vision,
        context_length: capabilities.context_length,
    })
}

/// Gives a conversation a title of its own, or takes one away.
///
/// An empty title is a clear rather than an error: the box the user typed in
/// starts out holding the derived title, and emptying it is how you say "go
/// back to that" — see [`sessions::rename`].
///
/// Allowed mid-turn, unlike deleting. A rename touches the transcript's header
/// and nothing a running turn is appending to, and stopping the turn to retitle
/// the conversation it is running in would be a strange thing to have to do.
#[tauri::command]
pub async fn rename_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    title: String,
) -> CmdResult<SessionMeta> {
    // The write is serialized against the log for this conversation when there
    // is one, so a rewrite cannot land between a turn's append and the next.
    // A conversation that is only on disk has nothing to serialize against.
    let held = state
        .session(&session_id)
        .ok()
        .map(|entry| entry.log.clone());
    let meta = match &held {
        Some(log) => {
            let _guard = log.lock().await;
            sessions::rename(&session_id, Some(&title))
        }
        None => sessions::rename(&session_id, Some(&title)),
    }?;

    info!(session = %session_id, title = %meta.title, "conversation renamed");
    emit_session(&state, &session_id).await;
    Ok(meta)
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct PermissionResponse {
    pub id: String,
    pub decision: PermissionDecision,
}

#[tauri::command]
pub async fn respond_permission(
    state: State<'_, Arc<AppState>>,
    response: PermissionResponse,
) -> CmdResult<()> {
    match state.pending_permissions.remove(&response.id) {
        Some((_, sender)) => {
            // Send failure means the waiting call already gave up; nothing to do.
            let _ = sender.send(response.decision);
            Ok(())
        }
        // Not an error: a turn that was canceled removes its own pending
        // requests, and the UI may answer a moment later.
        None => Ok(()),
    }
}

/// Answers a question card, releasing the tool call parked behind it.
///
/// One answer per question, in the order they were asked, and every one of them
/// may be empty — skipping is a first-class outcome, not a failure to respond.
/// See [`taurus_tools::view::Answer`].
#[tauri::command]
pub async fn answer_questions(
    state: State<'_, Arc<AppState>>,
    id: String,
    answers: Vec<Answer>,
) -> CmdResult<()> {
    match state.pending_questions.remove(&id) {
        Some((_, sender)) => {
            // Send failure means the call gave up first — a cancelled turn, or
            // a closed window. Nothing left to release.
            let _ = sender.send(answers);
            Ok(())
        }
        // Not an error, for the same reason `respond_permission` is not: a card
        // can still be on screen after the turn behind it was cancelled, and a
        // click on it should do nothing rather than raise a banner.
        None => Ok(()),
    }
}

/// The global provider layer, for the settings editor.
///
/// Deliberately not `AppStatus::providers`, which is the effective list with
/// this workspace's overrides folded in. An editor that saved that back would
/// write one project's settings into every project's config.
#[tauri::command]
pub async fn list_global_providers(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<ProviderConfig>> {
    Ok(state.host.global_providers().await)
}

/// Where each provider's API key comes from.
///
/// Status only — the key itself is never sent to the frontend. A secret handed
/// to the webview lives in JavaScript memory and in whatever the DOM does with
/// it, and there is nothing the settings screen needs it for: the field is a
/// place to type a new key, not to review the old one.
#[tauri::command]
pub async fn list_key_statuses(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<(String, KeyStatus)>> {
    Ok(state.host.key_statuses().await)
}

#[tauri::command]
pub async fn keychain_available() -> CmdResult<bool> {
    Ok(Host::keychain_available())
}

#[tauri::command]
pub async fn set_provider_key(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    key: String,
) -> CmdResult<()> {
    state.host.set_provider_key(&provider_id, &key).await?;
    Ok(())
}

#[tauri::command]
pub async fn clear_provider_key(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
) -> CmdResult<()> {
    state.host.clear_provider_key(&provider_id).await?;
    Ok(())
}

/// One search backend as the settings screen edits it.
///
/// Flattened out of `SearchFile`'s map so the frontend gets a list it can
/// render in a stable order, with the id alongside the entry rather than as a
/// key it has to carry separately.
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchBackend {
    pub id: String,
    pub kind: BackendKind,
    /// Empty when the config does not say and the kind has no default — which
    /// is only SearXNG, where guessing would turn a blank into a refused
    /// connection.
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub max_results: Option<u8>,
    /// False for SearXNG, so the UI can leave the key field out entirely.
    pub needs_key: bool,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct SearchSettings {
    /// The selected backend id, or null when search is off — which is the
    /// default, and not a problem.
    pub selected: Option<String>,
    pub backends: Vec<SearchBackend>,
    /// Where each backend's key comes from, by id. Status only; the key itself
    /// never crosses into the frontend.
    pub key_statuses: Vec<(String, KeyStatus)>,
    /// True when `web_search` is actually registered. A backend can be selected
    /// and still not run, and saying "on" then would be a lie.
    pub active: bool,
    pub problems: Vec<Problem>,
}

#[tauri::command]
pub async fn get_search_settings(state: State<'_, Arc<AppState>>) -> CmdResult<SearchSettings> {
    let file = state.host.global_search();

    let backends: Vec<SearchBackend> = file
        .backends
        .iter()
        .filter_map(|(id, entry)| {
            // An entry with no kind is a workspace override of something the
            // global layer never defined; there is nothing to edit here.
            let kind = entry.kind?;
            Some(SearchBackend {
                id: id.clone(),
                kind,
                base_url: entry
                    .base_url
                    .clone()
                    .or_else(|| kind.default_base_url().map(str::to_string))
                    .unwrap_or_default(),
                api_key_env: entry.api_key_env.clone(),
                max_results: entry.max_results,
                needs_key: kind.needs_key(),
            })
        })
        .collect();

    let key_statuses = backends
        .iter()
        .map(|b| {
            (
                b.id.clone(),
                taurus_host::config::search_key_status(&b.id, b.api_key_env.as_deref()),
            )
        })
        .collect();

    Ok(SearchSettings {
        selected: file.backend.clone(),
        backends,
        key_statuses,
        active: state.host.search_active().await,
        problems: state
            .host
            .problems_from(&[taurus_host::ProblemSource::Search])
            .await,
    })
}

#[tauri::command]
pub async fn save_search_settings(
    state: State<'_, Arc<AppState>>,
    selected: Option<String>,
    backends: Vec<SearchBackend>,
) -> CmdResult<()> {
    let mut file = state.host.global_search();

    // Entries are updated in place rather than rebuilt, so fields the settings
    // screen does not know about survive a save by someone who hand-edited the
    // file. Losing those silently is how a config editor earns distrust.
    for backend in &backends {
        let entry = file.backends.entry(backend.id.clone()).or_default();
        entry.kind = Some(backend.kind);
        entry.base_url = Some(backend.base_url.clone()).filter(|u| !u.trim().is_empty());
        entry.api_key_env = backend.api_key_env.clone().filter(|v| !v.trim().is_empty());
        entry.max_results = backend.max_results;
    }
    file.backend = selected.filter(|id| !id.trim().is_empty());

    state.host.set_search(file).await;
    Ok(())
}

#[tauri::command]
pub async fn set_search_key(
    state: State<'_, Arc<AppState>>,
    backend_id: String,
    key: String,
) -> CmdResult<()> {
    state.host.set_search_key(&backend_id, &key).await
}

#[tauri::command]
pub async fn clear_search_key(
    state: State<'_, Arc<AppState>>,
    backend_id: String,
) -> CmdResult<()> {
    state.host.clear_search_key(&backend_id).await
}

/// Opens a layer's `mcp.json` in whatever the OS uses for it, creating it first
/// if it is not there yet.
///
/// Still here now the panel exists, and deliberately. The format is the one
/// Claude Desktop uses and entries get pasted between the two, so the file has
/// to stay the authority: anything the panel cannot express — a key from a
/// newer version of the format, a comment, a server mid-edit — is edited here.
/// Every write the panel makes preserves what it does not understand, so the two
/// routes can be used interchangeably.
#[tauri::command]
pub async fn open_mcp_config(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    scope: Scope,
) -> CmdResult<String> {
    let workspace = state.host.workspace().await;
    let path = taurus_host::config::ensure_mcp_file(scope, Some(&workspace))?;

    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Every configured server, merged across layers, with how it is doing.
#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<McpServerView>> {
    // The same wait `get_status` takes, for the same reason: opening the panel
    // during the first load would otherwise render an empty list over a
    // `mcp.json` full of servers.
    state.loaded().await;
    Ok(state.host.mcp_servers().await)
}

/// Where the app looks for a stdio server's program, and what it took to get
/// there.
///
/// Shown in the panel rather than only logged. "Command not found" for a command
/// that plainly exists is the single most confusing thing this feature does, and
/// it is entirely explained by a PATH the user cannot otherwise see.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct McpEnvironment {
    /// The search directories, in order.
    pub path: Vec<String>,
    /// What the login shell contributed that the launcher did not. Empty when
    /// Taurus was started from a terminal, which is when it has nothing to add.
    pub added: Vec<String>,
    /// Why the shell was not asked, when it was not.
    #[ts(optional)]
    pub skipped: Option<String>,
}

#[tauri::command]
pub async fn mcp_environment() -> CmdResult<McpEnvironment> {
    // `adopt` ran at startup; this returns that same answer rather than probing
    // again, so the panel shows the PATH the servers were actually started with.
    let outcome = taurus_tools::login_path::adopt();
    Ok(McpEnvironment {
        path: taurus_tools::login_path::entries()
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        added: outcome.added.clone(),
        skipped: outcome.skipped.clone(),
    })
}

/// Writes one server into its layer's `mcp.json` and reconnects.
///
/// Reconnecting is part of the save rather than a second button. A panel that
/// saved and left the old connection running would be the bug this feature
/// started with — a server configured and not there — reproduced in the UI meant
/// to fix it.
///
/// `previous` is the entry being replaced, which the panel sends whenever it is
/// editing rather than adding. It is what a rename or a move between layers is
/// resolved against: the stored secrets are read from there, and it is removed
/// afterwards if the draft no longer lives at that name and scope.
#[tauri::command]
pub async fn save_mcp_server(
    state: State<'_, Arc<AppState>>,
    draft: McpServerDraft,
    previous: Option<McpServerRef>,
) -> CmdResult<Vec<McpServerView>> {
    let workspace = state.host.workspace().await;
    let source = previous.clone().unwrap_or_else(|| draft.origin());
    let server = draft.to_config(stored(&workspace, &source).as_ref());

    taurus_host::config::save_mcp_server(draft.scope, Some(&workspace), &draft.name, &server)?;

    // Only when it actually moved. Deleting unconditionally would make every
    // save a delete of the entry it had just written.
    if source.scope != draft.scope || source.name.trim() != draft.name.trim() {
        taurus_host::config::delete_mcp_server(source.scope, Some(&workspace), source.name.trim())?;
    }

    state.host.reload_mcp().await;
    // The panel is handed the listing directly; this is for the rail's badge,
    // which is showing the same servers from somewhere else on screen.
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

/// One layer's stored entry for a server, for the secrets the panel was never
/// given.
///
/// Read from the layer that entry actually lives in rather than from the merged
/// view: a workspace server must not silently inherit the global server's token
/// because the two share a name.
fn stored(workspace: &std::path::Path, entry: &McpServerRef) -> Option<taurus_mcp::ServerConfig> {
    taurus_host::config::scope_dir(entry.scope, Some(workspace))
        .and_then(|dir| taurus_mcp::load(&dir).ok())
        .and_then(|layer| layer.servers.get(entry.name.trim()).cloned())
}

#[tauri::command]
pub async fn delete_mcp_server(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    name: String,
) -> CmdResult<Vec<McpServerView>> {
    let workspace = state.host.workspace().await;
    taurus_host::config::delete_mcp_server(scope, Some(&workspace), &name)?;
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

#[tauri::command]
pub async fn set_mcp_server_disabled(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    name: String,
    disabled: bool,
) -> CmdResult<Vec<McpServerView>> {
    let workspace = state.host.workspace().await;
    taurus_host::config::set_mcp_server_disabled(scope, Some(&workspace), &name, disabled)?;
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

/// Connects to one entry, reports what it offers, and disconnects.
///
/// Takes the draft rather than a saved name, so an edit can be checked before it
/// is written. Nothing is registered and no live connection is touched.
#[tauri::command]
pub async fn test_mcp_server(
    state: State<'_, Arc<AppState>>,
    draft: McpServerDraft,
    previous: Option<McpServerRef>,
) -> CmdResult<Vec<String>> {
    let workspace = state.host.workspace().await;
    // A secret the form never had still has to reach the server being tested, or
    // testing an entry whose credential was not retyped would fail on the one
    // thing the panel deliberately never showed it. Resolved through `previous`
    // for the same reason a save is: a rename in the form must not lose the
    // token belonging to the entry it came from.
    let source = previous.unwrap_or_else(|| draft.origin());
    let server = draft.to_config(stored(&workspace, &source).as_ref());

    state.host.test_mcp_server(&draft.name, &server).await
}

/// Reconnects every MCP server without rescanning anything else.
///
/// Narrower than [`reload_config`] on purpose — see `Host::reload_mcp`.
#[tauri::command]
pub async fn reload_mcp(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<McpServerView>> {
    state.host.reload_mcp().await;
    emit_status(&state).await;
    Ok(state.host.mcp_servers().await)
}

#[tauri::command]
pub async fn list_permission_rules(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<AllowedRule>> {
    Ok(state.host.permissions().await.allowed_rules().await)
}

#[tauri::command]
pub async fn revoke_permission_rule(
    state: State<'_, Arc<AppState>>,
    rule: String,
    scope: Scope,
) -> CmdResult<()> {
    state.host.permissions().await.revoke(&rule, scope).await;
    Ok(())
}

#[tauri::command]
pub async fn list_skills(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<SkillSummary>> {
    // Rescanned first, the same as `list_agents`: the whole authoring surface
    // for a skill is a text editor and a folder, so a drawer showing the
    // catalog as it was at startup is not showing the feature working.
    state.host.rescan_skills().await;
    // The rail carries the count beside the drawer that lists them, and the two
    // disagreeing is worse than either being slightly late.
    emit_status(&state).await;
    Ok(state.host.skills().await)
}

/// The standing brief in force, in the order it reaches the prompt.
///
/// Shown beside the skill library because it belongs to the same question —
/// what is in the model's context before this conversation started — and
/// because a file being read silently is the kind of thing that makes
/// behaviour inexplicable. It is also where `ProblemSource::Instructions`
/// points, so a brief that did not load whole has somewhere to say so.
#[tauri::command]
pub async fn list_instructions(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<taurus_host::Instructions>> {
    Ok(state.host.instructions().await)
}

/// Skills and sub-agents the user can run as `/name`, for completion in the
/// composer.
///
/// A separate call from [`list_skills`] and [`list_agents`] rather than a merge
/// in the UI: which of either is user-invocable is the harness's answer to
/// give, and a composer offering one it would then refuse is a dead end typed
/// in full.
#[tauri::command]
pub async fn list_commands(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<taurus_host::CommandSummary>> {
    Ok(state.host.commands().await)
}

/// The sub-agent roster, rescanned from disk first.
///
/// Rescanning here rather than returning the cached catalog is what makes the
/// drawer show the feature working: the whole authoring surface is a text
/// editor, so a list assembled at startup is stale by the time anyone opens it.
#[tauri::command]
pub async fn list_agents(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<AgentSummary>> {
    state.host.rescan_agents().await;
    // Same reason the skills listing does it: the rail's count and the drawer's
    // list are two views of one scan and must not disagree.
    emit_status(&state).await;
    Ok(state.host.agents().await)
}

/// What earlier conversations in this workspace wrote down for the next one.
///
/// Shown for the same reason the standing brief is: this is part of what is in
/// the model's context before a conversation starts, and context the user
/// cannot see is behaviour they cannot explain. Unlike the brief, they can also
/// take a line out of it.
#[tauri::command]
pub async fn list_notes(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<Note>> {
    Ok(state.host.notes().await)
}

#[tauri::command]
pub async fn list_datasets(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<Dataset>> {
    Ok(state.host.datasets().await)
}

/// Reads a dataset in full and describes every column.
///
/// The one command here that can take real time — it is a scan of the whole
/// file — which is why the pane shows a reading state over it rather than
/// waiting in silence.
#[tauri::command]
pub async fn dataset_profile(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> CmdResult<DataProfile> {
    state.host.dataset_profile(&name).await
}

/// A window of a dataset's rows.
///
/// `limit` is clamped by the engine rather than trusted, so a frontend asking
/// for the whole file gets a page and not a hang. See `taurus_data::MAX_PAGE`.
#[tauri::command]
pub async fn dataset_page(
    state: State<'_, Arc<AppState>>,
    name: String,
    offset: u64,
    limit: u64,
) -> CmdResult<DataPage> {
    state.host.dataset_page(&name, offset, limit).await
}

/// Answers one read-only SQL question over every dataset loaded here.
///
/// The engine refuses anything that is not a read, which is what lets this be
/// a plain command rather than one behind a confirmation: the box in the pane
/// takes arbitrary text, and `COPY … TO` is one line of SQL.
#[tauri::command]
pub async fn query_data(
    state: State<'_, Arc<AppState>>,
    sql: String,
) -> CmdResult<DataQueryResult> {
    state.host.query_data(&sql).await
}

/// Every recipe this workspace has, with anything wrong with the rest.
///
/// Problems travel with the list rather than as a failure. A recipe that will
/// not parse is a file somebody is halfway through writing, and hiding the
/// other four while they finish would be the pane arguing with an editor.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct Recipes {
    pub recipes: Vec<Recipe>,
    /// One line per unreadable file, each naming its path.
    pub problems: Vec<String>,
}

#[tauri::command]
pub async fn list_recipes(state: State<'_, Arc<AppState>>) -> CmdResult<Recipes> {
    let (recipes, problems) = state.host.recipes().await;
    Ok(Recipes { recipes, problems })
}

/// Runs a recipe and writes the file it names.
///
/// The one command in this family that changes the workspace, and it does it
/// without a permission prompt for the same reason `query_data` does not have
/// one: the person clicked the button, and the button says what it writes.
/// What the engine still refuses is a *step* that writes somewhere else — the
/// button named one path and only that path was agreed to.
#[tauri::command]
pub async fn run_recipe(state: State<'_, Arc<AppState>>, name: String) -> CmdResult<DataRun> {
    let run = state.host.run_recipe(&name).await?;
    // A recipe's output is loaded as a dataset on the way out, so the pane's
    // list and the rail's count have both just changed.
    emit_status(&state).await;
    Ok(run)
}

/// Drops a dataset from the list and answers with what is left.
///
/// The file it pointed at is not touched. Forgetting a dataset is the opposite
/// of a destructive act — it is the way to correct a mistaken load — so it
/// arms no confirmation, unlike deleting a conversation.
#[tauri::command]
pub async fn forget_dataset(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> CmdResult<Vec<Dataset>> {
    let left = state.host.forget_dataset(&name).await?;
    // The tab disappears when the last one goes, so the rest of the window has
    // to hear about it.
    emit_status(&state).await;
    Ok(left)
}

/// Drops one note and answers with what is left, so the drawer redraws from the
/// file rather than from its own guess about what the file now says.
#[tauri::command]
pub async fn forget_note(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<Vec<Note>> {
    let left = state.host.forget_note(&id).await?;
    // The drawer is handed the remaining notes directly; this is for the count
    // on the rail behind it.
    emit_status(&state).await;
    Ok(left)
}

/// Every tool this session has, for the editor's tool picker.
///
/// The live registry rather than a compiled-in list, so a skill or MCP tool
/// approved earlier in the session is offered — and so an agent cannot be
/// scoped to a tool that would be refused the moment it was saved.
#[tauri::command]
pub async fn list_tools(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<String>> {
    Ok(state.host.tool_names().await)
}

/// Saves an agent written in the editor.
///
/// The same path an approved proposal takes — same validation, same writer,
/// same rescan — because a hand-written agent and a generated one are the same
/// file, and two writers would drift.
#[tauri::command]
pub async fn save_agent(
    state: State<'_, Arc<AppState>>,
    draft: AgentProposal,
    target: AgentSaveTarget,
) -> CmdResult<String> {
    let available: Vec<String> = state
        .host
        .registry()
        .read()
        .await
        .names()
        .map(str::to_string)
        .collect();
    {
        let catalog = state.host.agent_catalog().read().await;
        validate_agent(&draft, &catalog, &available)
            .map_err(|e| format!("this agent cannot be saved as written: {e}"))?;
    }

    let root = match target {
        AgentSaveTarget::Project => {
            taurus_host::config::workspace_agents_dir(&state.host.workspace().await)
        }
        AgentSaveTarget::User => taurus_host::config::user_agents_dir(),
    };
    let path = save_agent_file(&draft, &root).map_err(|e| format!("could not save agent: {e}"))?;
    info!(agent = %draft.name, path = %path.display(), "agent saved from the editor");

    state.host.rescan_agents().await;
    emit_status(&state).await;
    Ok(path.display().to_string())
}

/// What the model is told to produce for the editor's Generate button.
///
/// Named fields rather than "write an agent file": the editor owns the format,
/// and a model asked for YAML frontmatter returns YAML that is *nearly* right
/// often enough to matter. JSON it can be checked against, and every field
/// lands in a box the user can correct.
///
/// Built rather than declared, so the range it quotes is the range the loader
/// actually enforces. Stated as a literal it once drifted from the ceiling the
/// moment that ceiling moved, and a model told the wrong range returns drafts
/// that are silently clamped on the way in.
fn draft_system() -> String {
    format!(
        "\
You draft a sub-agent definition for a coding agent, and reply with JSON only — \
no prose, no markdown fence.

Fields:
- name: kebab-case, specific, e.g. \"migration-checker\"
- description: one sentence under {DESCRIPTION_LIMIT} characters saying when to \
delegate here. This is the only text the caller sees when choosing, so describe \
the job.
- tools: an array chosen ONLY from the allowed list you are given, or null to \
inherit the caller's tools. Pick the narrowest set that can do the work.
- max_iterations: 1 to {MAX_ITERATIONS_LIMIT}. Around 20 for work that only \
reads, 25 if it writes. Reach past 50 only for work that genuinely cannot \
finish in fewer rounds.
- prompt: the agent's system prompt. It shares none of the caller's context and \
cannot ask questions, so say what to do, what not to do, and what to report \
back. Several sentences."
    )
}

/// Drafts an agent from a description, for the editor to fill in.
///
/// A one-shot completion rather than a turn: there are no tools to call and
/// nothing to undo, and running it through the agent loop would put a draft
/// nobody asked for into the transcript.
///
/// Whatever comes back is a starting point, not a result — it lands in the
/// editor's fields, and the user is the one who saves it. So this repairs what
/// it can (an out-of-range iteration count, a tool the session lacks) rather
/// than refusing a draft over something the user can see and fix.
#[tauri::command]
pub async fn generate_agent(
    state: State<'_, Arc<AppState>>,
    description: String,
    provider_id: String,
    model: String,
) -> CmdResult<AgentProposal> {
    if description.trim().is_empty() {
        return Err("describe what the agent should do first".into());
    }

    let available: Vec<String> = state
        .host
        .registry()
        .read()
        .await
        .names()
        .map(str::to_string)
        .collect();

    let provider = state.host.provider(&provider_id).await?;
    let mut request = ChatRequest::new(
        &model,
        vec![Message::user(format!(
            "Draft a sub-agent for: {}\n\nAllowed tools: {}",
            description.trim(),
            available.join(", ")
        ))],
    );
    request.system = Some(draft_system());

    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let handle = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

    let mut acc = StreamAccumulator::new();
    while let Some(event) = rx.recv().await {
        acc.push(event);
    }
    handle
        .await
        .map_err(|e| format!("the draft did not finish: {e}"))?
        .map_err(|e| format!("could not reach {provider_id}: {e}"))?;

    let text = acc.finish().0.text();
    let json = extract_json(&text).ok_or_else(|| {
        format!(
            "{model} did not answer with JSON. It said: {}",
            brief(&text)
        )
    })?;
    let drafted: DraftedAgent = serde_json::from_str(json)
        .map_err(|e| format!("{model} answered with JSON that does not fit an agent: {e}"))?;

    let mut proposal = AgentProposal::new(
        drafted.name.trim(),
        drafted.description.trim(),
        drafted.prompt.trim(),
    );
    proposal.max_iterations = drafted
        .max_iterations
        .unwrap_or(20)
        .clamp(1, MAX_ITERATIONS_LIMIT);
    // Silently dropped rather than refused: a model naming a tool that does not
    // exist here has still drafted a usable agent, and the picker below shows
    // exactly what survived.
    proposal.tools = drafted.tools.map(|tools| {
        tools
            .into_iter()
            .filter(|tool| available.contains(tool))
            .collect()
    });
    // An empty list means "no tools" to the loader and "everything" to nobody.
    // If filtering emptied it, inheriting is the honest reading of a draft that
    // named only tools this session lacks.
    if proposal.tools.as_ref().is_some_and(Vec::is_empty) {
        proposal.tools = None;
    }

    info!(agent = %proposal.name, "agent drafted");
    Ok(proposal)
}

#[derive(Deserialize)]
struct DraftedAgent {
    name: String,
    description: String,
    prompt: String,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    max_iterations: Option<u32>,
}

/// The first JSON object in a reply.
///
/// Models wrap JSON in prose and markdown fences however often they are asked
/// not to, and a draft thrown away over a fence is a round trip spent on
/// punctuation. Brace-counting rather than a regex because the prompt itself
/// contains braces.
fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Enough of a reply to recognize it, for an error message.
fn brief(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.chars().count() > 160 {
        true => format!("{}…", trimmed.chars().take(160).collect::<String>()),
        false => trimmed.to_string(),
    }
}

/// What the roster costs on every request, in characters. Shown beside it,
/// because an expense nobody can see is one nobody chose.
#[tauri::command]
pub async fn agent_roster_cost(state: State<'_, Arc<AppState>>) -> CmdResult<usize> {
    Ok(state.host.roster_cost().await)
}

/// Writes a starter agent file and opens it.
///
/// Disk stays the source of truth — there is no in-app editor, deliberately —
/// but nobody should have to already know the frontmatter to write their first
/// agent. The template documents every key in place.
#[tauri::command]
pub async fn create_agent(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    name: String,
) -> CmdResult<String> {
    let workspace = state.host.workspace().await;
    let path = taurus_host::config::create_agent_file(scope, Some(&workspace), &name)?;
    state.host.rescan_agents().await;
    emit_status(&state).await;

    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn list_proposals(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<SkillProposal>> {
    Ok(state
        .pending_proposals
        .iter()
        .map(|e| e.value().clone())
        .collect())
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct ProposalResponse {
    pub id: String,
    pub approve: bool,
    /// Where to save it. Ignored when rejecting.
    #[serde(default)]
    pub target: Option<SaveTarget>,
    /// The user's edits, if they changed anything in the review card.
    #[serde(default)]
    pub edited: Option<SkillProposal>,
}

#[tauri::command]
pub async fn respond_skill_proposal(
    state: State<'_, Arc<AppState>>,
    response: ProposalResponse,
) -> CmdResult<Option<String>> {
    let Some((_, original)) = state.pending_proposals.remove(&response.id) else {
        return Err(format!("no pending proposal '{}'", response.id));
    };

    if !response.approve {
        info!(skill = %original.name, "skill proposal rejected");
        return Ok(None);
    }

    let proposal = response.edited.unwrap_or(original);
    let root = match response.target.unwrap_or(SaveTarget::Project) {
        SaveTarget::Project => {
            taurus_host::config::workspace_skills_dir(&state.host.workspace().await)
        }
        SaveTarget::User => taurus_host::config::user_skills_dir(),
    };

    let dir = save(&proposal, &root).map_err(|e| format!("could not save skill: {e}"))?;
    info!(skill = %proposal.name, dir = %dir.display(), "skill approved");

    // Reload so the skill is usable in the session that just proposed it.
    state.host.reload().await;
    // And so the count on the rail moves with it, rather than on whatever the
    // user does next.
    emit_status(&state).await;
    Ok(Some(dir.display().to_string()))
}

/// Retunes one agent's iteration limit. Returns the file that now holds it,
/// which for a built-in is an override that did not exist a moment ago.
#[tauri::command]
pub async fn set_agent_iterations(
    state: State<'_, Arc<AppState>>,
    name: String,
    limit: u32,
) -> CmdResult<String> {
    state.host.set_agent_iterations(&name, limit).await
}

#[tauri::command]
pub async fn set_max_iterations(state: State<'_, Arc<AppState>>, limit: u32) -> CmdResult<()> {
    state.host.set_max_iterations(limit).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_skill_synthesis(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    state.host.set_skill_synthesis(enabled).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn list_agent_proposals(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<AgentProposal>> {
    Ok(state
        .pending_agent_proposals
        .iter()
        .map(|e| e.value().clone())
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProposalResponse {
    pub id: String,
    pub approve: bool,
    /// Where to save it. Ignored when rejecting.
    #[serde(default)]
    pub target: Option<AgentSaveTarget>,
    /// The user's edits, if they changed anything in the review card.
    #[serde(default)]
    pub edited: Option<AgentProposal>,
}

#[tauri::command]
pub async fn respond_agent_proposal(
    state: State<'_, Arc<AppState>>,
    response: AgentProposalResponse,
) -> CmdResult<Option<String>> {
    let Some((_, original)) = state.pending_agent_proposals.remove(&response.id) else {
        return Err(format!("no pending agent proposal '{}'", response.id));
    };

    if !response.approve {
        info!(agent = %original.name, "agent proposal rejected");
        return Ok(None);
    }

    let proposal = response.edited.unwrap_or(original);

    // Re-validated because the card is editable. What the model proposed passed
    // on the way in; what the user is about to save may be something else
    // entirely, and a hand-edited name or tool list has never been checked.
    let available: Vec<String> = state
        .host
        .registry()
        .read()
        .await
        .names()
        .map(str::to_string)
        .collect();
    {
        let catalog = state.host.agent_catalog().read().await;
        validate_agent(&proposal, &catalog, &available)
            .map_err(|e| format!("this agent cannot be saved as written: {e}"))?;
    }

    let root = match response.target.unwrap_or(AgentSaveTarget::Project) {
        AgentSaveTarget::Project => {
            taurus_host::config::workspace_agents_dir(&state.host.workspace().await)
        }
        AgentSaveTarget::User => taurus_host::config::user_agents_dir(),
    };

    let path =
        save_agent_file(&proposal, &root).map_err(|e| format!("could not save agent: {e}"))?;
    info!(agent = %proposal.name, path = %path.display(), "agent approved");

    // Narrower than `reload`, which would restart every MCP server to pick up
    // one markdown file.
    state.host.rescan_agents().await;
    emit_status(&state).await;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub async fn set_agent_synthesis(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    state.host.set_agent_synthesis(enabled).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_theme(state: State<'_, Arc<AppState>>, theme: Theme) -> CmdResult<()> {
    state.host.set_theme(theme).await;
    // The settings file stays the authority on which theme is in force, and
    // this is how the window is told what it now says.
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_embedding_model(
    state: State<'_, Arc<AppState>>,
    model: String,
    provider: String,
) -> CmdResult<()> {
    state.host.set_embedding_model(&model, &provider).await;
    // The tool is registered by `reload`, so without this a model named here
    // does not become a `search_code` until the next workspace change — which
    // reads as the setting not having taken.
    state.host.reload().await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_rerank(
    state: State<'_, Arc<AppState>>,
    model: String,
    provider: String,
) -> CmdResult<()> {
    state.host.set_rerank(&model, &provider).await;
    // Same reason `set_embedding_model` reloads: the reranker is attached to
    // `search_code` when the tool is registered, so without this it does not
    // take hold until the next workspace change.
    state.host.reload().await;
    emit_status(&state).await;
    Ok(())
}

/// How far through a build is, as the UI draws it.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct IndexProgress {
    pub done: usize,
    pub total: usize,
}

/// Builds this workspace's semantic index now, rather than inside the first
/// turn that reaches for it.
///
/// The whole point is that it is not a turn: the first index of a repository
/// takes the better part of a minute, and paying it here means paying it
/// against a progress bar that someone chose to start, instead of inside a tool
/// call that has not returned.
///
/// A `Channel` for the same reason `send_message` uses one — delivery is
/// ordered and scoped to this call, so a second window building a different
/// workspace cannot interleave into this one's bar.
#[tauri::command]
pub async fn build_index(
    state: State<'_, Arc<AppState>>,
    on_progress: Channel<IndexProgress>,
) -> CmdResult<String> {
    struct ToChannel(Channel<IndexProgress>);

    #[async_trait::async_trait]
    impl taurus_host::IndexProgress for ToChannel {
        async fn embedding(&self, done: usize, total: usize) {
            // A dropped channel means the settings pane closed. The build is
            // still worth finishing — the index is the point, not the bar.
            let _ = self.0.send(IndexProgress { done, total });
        }
    }

    // Its own token rather than a session's: this belongs to no conversation,
    // and cancelling it must not cancel a turn that happens to be running.
    let cancel = CancellationToken::new();
    *state.index_build.lock().await = cancel.clone();

    state
        .host
        .build_index(cancel, Some(&ToChannel(on_progress)))
        .await
}

/// Stops a running index build. Safe to call when none is running.
#[tauri::command]
pub async fn stop_index_build(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.index_build.lock().await.cancel();
    Ok(())
}

#[tauri::command]
pub async fn save_providers(
    state: State<'_, Arc<AppState>>,
    providers: Vec<ProviderConfig>,
) -> CmdResult<()> {
    if providers.is_empty() {
        return Err("at least one provider must be configured".into());
    }
    state.host.set_providers(providers).await;
    emit_status(&state).await;
    Ok(())
}

/// Re-reads both config layers, rescans skills and agents, and reconnects MCP
/// servers.
///
/// Named for what it does. It was `reload_skills`, which promised less than it
/// delivered from the day agents were discovered by the same call: someone who
/// had edited an agent would not press it, and someone who pressed it would not
/// expect their agent edits to land.
#[tauri::command]
pub async fn reload_config(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.host.reload().await;
    // The broadest of these: every count, every server and every problem the
    // shell shows is re-read by that call.
    emit_status(&state).await;
    Ok(())
}

/// Re-reads the files a person edits in an editor: instructions, skills,
/// sub-agents, hooks.
///
/// What returning to the window calls. These are the four things somebody
/// writes in another application and then expects to find here, and coming back
/// from that application is the closest thing to an event their arrival has —
/// nothing watches those directories, deliberately.
///
/// The same gate a turn uses, so this is a `stat` per file when nothing moved
/// rather than a rescan on every alt-tab. Much narrower than [`reload_config`]
/// either way: that one also re-reads both provider layers and reconnects every
/// MCP server.
///
/// Refuses nothing, but the caller is expected not to ask mid-turn — see
/// `Host::refresh_config`.
#[tauri::command]
pub async fn rescan_library(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.host.refresh_config().await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn list_checkpoints(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> CmdResult<Vec<Checkpoint>> {
    // `turns` deserializes the whole checkpoint log, and a `Before` record
    // carries the full pre-image of every file the turn touched — all of which
    // this then throws away, keeping only the names. The cost is (turns × files
    // × file size) rather than anything the drawer shows, so it stays off the
    // runtime until the log grows a lighter header to read instead.
    let store = state
        .host
        .checkpoints_for(&session_workspace(&state, &session_id).await);
    off_runtime(move || store.turns(&session_id)).await
}

/// Restores the workspace to just before `turn`.
///
/// With `dry_run`, reports what that would do and writes nothing — which is how
/// the UI shows the plan before asking.
#[tauri::command]
pub async fn rewind_to(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    turn: u32,
    dry_run: bool,
) -> CmdResult<Rewind> {
    // A turn holds this lock for its whole run. Rewinding underneath one would
    // race the tool calls still writing, and the disabled button in the UI is
    // not something the backend should have to trust.
    if let Ok(entry) = state.session(&session_id) {
        if entry.session.try_lock().is_err() {
            return Err("this conversation is mid-turn; stop it before rewinding".into());
        }
    }

    // The conversation's own folder, which is where its pre-images came from.
    // Resolved against the open one instead, a rewind reached for a log that
    // is not there and reported nothing to undo — and had it found one, it
    // would have restored a different project's files.
    let workspace = session_workspace(&state, &session_id).await;
    let rewind =
        state
            .host
            .checkpoints_for(&workspace)
            .rewind(&session_id, &workspace, turn, dry_run)?;

    // The checklist is working state, and rewinding is undoing the work it
    // tracked. Kept, it would be the one thing in the session that still
    // believes in a turn nothing else remembers — the model reading a plan for
    // files that have been put back. A dry run changes nothing, so it clears
    // nothing.
    if !dry_run {
        state.host.forget_plan(&session_id).await;
        // The header counts files this conversation changed, and a rewind is
        // the only thing that makes that number go down. The drawer re-reads
        // its own list; this is for the count behind it.
        emit_changed(&state, &session_id).await;
    }
    Ok(rewind)
}

/// What one turn changed, file by file, as a diff.
///
/// Read on demand rather than sent with the listing: a session of thirty turns
/// would otherwise ship every diff it ever made to draw a drawer, and the
/// drawer shows one turn expanded at a time.
#[tauri::command]
pub async fn turn_changes(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    turn: u32,
) -> CmdResult<Vec<TurnChange>> {
    let workspace = session_workspace(&state, &session_id).await;
    state
        .host
        .checkpoints_for(&workspace)
        .changes(&session_id, &workspace, turn)
}

/// Where the workspace stands with git, for the branch label and the commit
/// button.
#[tauri::command]
pub async fn repo_status(state: State<'_, Arc<AppState>>) -> CmdResult<RepoStatus> {
    Ok(state.host.repo_status().await)
}

/// Commits exactly the files one turn changed.
///
/// The turn is named rather than the paths, so the frontend cannot ask for a
/// commit of files that turn did not touch — the checkpoint log is the only
/// thing that decides what goes in.
#[tauri::command]
pub async fn commit_turn(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    turn: u32,
    message: String,
) -> CmdResult<Commit> {
    // The same rule `rewind_to` applies: a turn holds this lock for its whole
    // run, and committing underneath one would capture a tree that is still
    // being written.
    if let Ok(entry) = state.session(&session_id) {
        if entry.session.try_lock().is_err() {
            return Err(
                "this conversation is mid-turn; wait for it to finish before committing".into(),
            );
        }
    }

    // The conversation's own folder, which is the repository its turns changed
    // files in — not whichever one is open now.
    let workspace = session_workspace(&state, &session_id).await;
    let checkpoints = state.host.checkpoints_for(&workspace);

    // Re-read rather than trusting a path list from the frontend, so what is
    // committed is what was recorded.
    let files = checkpoints
        .turns(&session_id)?
        .into_iter()
        .find(|checkpoint| checkpoint.turn == turn)
        .map(|checkpoint| checkpoint.files)
        .ok_or_else(|| format!("turn {turn} is not in this conversation's checkpoint log"))?;

    let repo = Repo::discover(&workspace)
        .await?
        .ok_or("This workspace is not a git repository, so there is nothing to commit to.")?;

    let commit = repo.commit(&files, &message).await?;

    // After the commit, because until git has made one there is no sha to write
    // down. This is what lets the drawer say which turns are already in `HEAD`
    // — so a commit of turn 5 can warn that turn 4 is not in one, and a rewind
    // past turn 3 can say the commit it is about to orphan.
    checkpoints.record_commit(&session_id, &workspace, turn, &commit.sha);

    info!(
        session = %session_id,
        turn,
        sha = %commit.sha,
        files = commit.files.len(),
        skipped = commit.skipped.len(),
        "committed a turn"
    );
    Ok(commit)
}

/* ------------------------------------------------------------- terminal */

/// Starts a shell and streams it to the pane that asked.
///
/// `cwd` is the workspace root when the pane does not name one, which is what
/// it wants on the first open: a terminal that starts in the home directory
/// beside a window that is looking at a project is one `cd` away from being
/// useful and nobody remembers to type it.
///
/// The size is the pane's, measured after it is laid out. A terminal opened at
/// a guessed size and corrected a moment later shows the shell's first prompt
/// wrapped at the wrong column, which is the one artifact of a resize that does
/// not redraw away.
#[tauri::command]
pub async fn terminal_open(
    state: State<'_, Arc<AppState>>,
    cwd: Option<String>,
    rows: u16,
    cols: u16,
    on_event: Channel<TerminalEvent>,
) -> CmdResult<String> {
    let root = state.host.workspace().await;
    let cwd = match cwd {
        Some(path) => PathBuf::from(path),
        None => root.clone(),
    };
    // A folder that has been renamed or unmounted under the window would
    // otherwise fail inside the spawn as a message about the shell, which is
    // the wrong thing to name.
    let cwd = if cwd.is_dir() { cwd } else { root };
    state.terminals.open(&cwd, rows, cols, on_event)
}

/// Sends keystrokes. `data` is the text the emulator produced, escape
/// sequences and all — arrow keys and Ctrl chords arrive here as the bytes a
/// terminal would have sent.
#[tauri::command]
pub async fn terminal_write(
    state: State<'_, Arc<AppState>>,
    id: String,
    data: String,
) -> CmdResult<()> {
    state.terminals.write(&id, data.as_bytes())
}

/// Tells the shell how big its window is now.
///
/// This is what makes a full-screen program redraw at the new size, and it is
/// also the only thing that tells a shell where to wrap. A pane that resizes
/// without saying so leaves every program inside it drawing to the old
/// geometry.
#[tauri::command]
pub async fn terminal_resize(
    state: State<'_, Arc<AppState>>,
    id: String,
    rows: u16,
    cols: u16,
) -> CmdResult<()> {
    state.terminals.resize(&id, rows, cols)
}

/// Ends a shell, and anything it is running.
#[tauri::command]
pub async fn terminal_close(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<()> {
    state.terminals.close(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_turn_in_the_folder_the_conversation_belongs_to_is_allowed() {
        assert!(check_workspace(Path::new("/src/a"), Path::new("/src/a")).is_ok());
    }

    #[test]
    fn a_turn_from_another_folder_is_refused_and_told_where_to_go() {
        // Two checkouts of one project is the case the path has to be spelled
        // out for: "wrong workspace" would not say which of them.
        let err = check_workspace(Path::new("/src/a/taurus"), Path::new("/work/taurus"))
            .expect_err("a conversation must not be continued from another folder");
        assert!(err.contains("/src/a/taurus"), "{err}");
        assert!(
            !err.contains("/work/taurus"),
            "names the way back, not the dead end: {err}"
        );
    }
}
