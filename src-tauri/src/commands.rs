//! Tauri commands: the entire surface the frontend can reach.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use ts_rs::TS;

use taurus_core::{Session, UiEvent};
use taurus_mcp::ServerStatus;
use taurus_provider::{Message, ModelInfo};
use taurus_skills::proposal::{save, SaveTarget, SkillProposal};
use taurus_skills::skill::SkillSummary;
use taurus_tools::{AllowedRule, PermissionDecision, Scope};

use taurus_host::{
    sessions, BackendKind, Checkpoint, Host, KeyStatus, Problem, ProviderConfig, Restored,
    SessionLog, SessionMeta, Settings, Theme, TurnRef,
};

use crate::state::{AppState, SessionEntry};

/// Commands return this so the frontend gets a readable message rather than a
/// serialized Rust error.
pub type CmdResult<T> = Result<T, String>;

async fn session_model(entry: &Arc<SessionEntry>) -> String {
    entry.session.lock().await.model.clone()
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct AppStatus {
    pub workspace: String,
    pub providers: Vec<ProviderConfig>,
    pub settings: Settings,
    pub skill_count: usize,
    /// Everything that failed to load, each tagged with where it came from so
    /// the UI can show it on the screen that can fix it. Previously this was an
    /// untagged list called `skill_problems`, and a malformed `providers.json`
    /// was reported under a list of skills.
    pub problems: Vec<Problem>,
    pub tool_names: Vec<String>,
    pub mcp_servers: Vec<ServerStatus>,
}

#[tauri::command]
pub async fn get_status(state: State<'_, Arc<AppState>>) -> CmdResult<AppStatus> {
    Ok(AppStatus {
        workspace: state.host.workspace().await.display().to_string(),
        providers: state.host.providers().await,
        settings: state.host.settings().await,
        skill_count: state.host.skill_count().await,
        problems: state.host.problems().await,
        tool_names: state.host.tool_names().await,
        mcp_servers: state.host.mcp_statuses().await,
    })
}

#[tauri::command]
pub async fn set_workspace(state: State<'_, Arc<AppState>>, path: String) -> CmdResult<String> {
    let resolved = state.host.set_workspace(&PathBuf::from(path)).await?;
    info!(workspace = %resolved.display(), "workspace changed");
    Ok(resolved.display().to_string())
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
    let log = SessionLog::create(&session, &state.host.workspace().await);
    let id = session.id.clone();
    state.sessions.insert(
        id.clone(),
        Arc::new(SessionEntry {
            session: Arc::new(Mutex::new(session)),
            provider_id: provider_id.clone(),
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            log: Arc::new(Mutex::new(log)),
        }),
    );

    state.host.remember_session(&provider_id, &model).await;

    info!(session = %id, %model, native_tools = capabilities.native_tools, "session created");
    Ok(CreatedSession {
        id,
        model,
        provider_id,
        native_tools: capabilities.native_tools,
        context_length: capabilities.context_length,
    })
}

/// Saved conversations, newest first — this workspace's, or every one.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, Arc<AppState>>,
    all: bool,
) -> CmdResult<Vec<SessionMeta>> {
    let workspace = state.host.workspace().await;
    Ok(sessions::list(if all { None } else { Some(&workspace) }))
}

/// What a resumed conversation needs to be redrawn and continued.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct ResumedSession {
    pub id: String,
    pub model: String,
    pub provider_id: String,
    pub native_tools: bool,
    pub context_length: u32,
    /// The whole transcript, for the frontend to rebuild the view from.
    pub messages: Vec<Message>,
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
    let (session, provider_id) = match state.sessions.get(&session_id) {
        Some(open) => {
            let entry = open.clone();
            let provider_id = entry.provider_id.clone();
            let session = entry.session.lock().await.clone();
            (session, provider_id)
        }
        None => {
            let (session, _) = sessions::load(&session_id)?;
            // Whichever provider the caller is on, else whatever the host
            // resolves: a transcript records the model, not the backend that
            // served it, and that backend may not even be configured now.
            let (resolved, _) = state
                .host
                .resolve_model(provider_id.as_deref(), Some(&session.model))
                .await?;
            (session, resolved)
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
        context_length: capabilities.context_length,
        messages: session.messages.clone(),
    };

    if !state.sessions.contains_key(&session_id) {
        let log = SessionLog::resume(&session, &state.host.workspace().await);
        state.sessions.insert(
            session_id.clone(),
            Arc::new(SessionEntry {
                session: Arc::new(Mutex::new(session)),
                provider_id,
                cancel: Arc::new(Mutex::new(CancellationToken::new())),
                log: Arc::new(Mutex::new(log)),
            }),
        );
        info!(session = %session_id, "session resumed");
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
    on_event: Channel<UiEvent>,
) -> CmdResult<()> {
    let entry = state.session(&session_id)?;
    let provider = state.host.provider(&entry.provider_id).await?;

    // A fresh token per turn: reusing a canceled one would abort the next turn
    // before it started.
    let cancel = CancellationToken::new();
    *entry.cancel.lock().await = cancel.clone();

    let model = session_model(&entry).await;
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
        .await;

    // Bridge the loop's mpsc channel to the IPC channel.
    let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
    let forwarder = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if on_event.send(event).is_err() {
                break;
            }
        }
    });

    let mut session = entry.session.lock().await;
    let outcome = agent.run_turn(&mut session, Message::user(text), tx).await;
    // Recorded whatever the outcome, and before the session lock is released:
    // an interrupted turn still produced the messages that led there, and they
    // must reach disk in the order they happened.
    entry.log.lock().await.record(&session);
    drop(session);
    let _ = forwarder.await;

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
    info!(session = %session_id, "session deleted");
    Ok(())
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
/// MCP servers are configured by editing that file — the format is the one
/// Claude Desktop uses, and people move entries between the two — so this is a
/// route to the file rather than a form that would have to be kept in step with
/// a schema Taurus does not own.
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
    Ok(state.host.skills().await)
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
    Ok(Some(dir.display().to_string()))
}

#[tauri::command]
pub async fn set_skill_synthesis(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    state.host.set_skill_synthesis(enabled).await;
    Ok(())
}

#[tauri::command]
pub async fn set_theme(state: State<'_, Arc<AppState>>, theme: Theme) -> CmdResult<()> {
    state.host.set_theme(theme).await;
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
    Ok(())
}

#[tauri::command]
pub async fn reload_skills(state: State<'_, Arc<AppState>>) -> CmdResult<usize> {
    state.host.reload().await;
    Ok(state.host.skill_count().await)
}

#[tauri::command]
pub async fn list_checkpoints(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> CmdResult<Vec<Checkpoint>> {
    state.host.checkpoints().await.turns(&session_id)
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
) -> CmdResult<Vec<Restored>> {
    // A turn holds this lock for its whole run. Rewinding underneath one would
    // race the tool calls still writing, and the disabled button in the UI is
    // not something the backend should have to trust.
    if let Ok(entry) = state.session(&session_id) {
        if entry.session.try_lock().is_err() {
            return Err("this conversation is mid-turn; stop it before rewinding".into());
        }
    }

    let workspace = state.host.workspace().await;
    state
        .host
        .checkpoints()
        .await
        .rewind(&session_id, &workspace, turn, dry_run)
}
