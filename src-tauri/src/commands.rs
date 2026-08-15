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

use taurus_agents::proposal::{
    save as save_agent_file, validate_proposal as validate_agent, AgentProposal,
    SaveTarget as AgentSaveTarget,
};
use taurus_agents::AgentSummary;
use taurus_core::{Session, UiEvent};
use taurus_mcp::ServerStatus;
use taurus_provider::{ChatRequest, Message, ModelInfo, StreamAccumulator};
use taurus_skills::proposal::{save, SaveTarget, SkillProposal};
use taurus_skills::skill::SkillSummary;
use taurus_tools::{AllowedRule, Answer, PermissionDecision, Scope};

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
    /// The roster size, so the rail can carry it beside the skill count. Never
    /// zero: two agents ship with the harness.
    pub agent_count: usize,
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
        agent_count: state.host.agents().await.len(),
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

    // `/name args` becomes the skill's procedure before the model sees it. The
    // user's own line stays what the transcript shows and what names the turn:
    // an expansion is how the request is carried out, not what was asked.
    let prompt = match state.host.expand_command(&text).await {
        Some(Ok(invocation)) => {
            info!(session = %session_id, skill = %invocation.name, "ran a skill as a command");
            invocation.prompt
        }
        // Returned rather than sent. A mistyped command is a message to the
        // user, and passing it to the model would answer a question about a
        // skill instead of running one.
        Some(Err(e)) => return Err(e.to_string()),
        None => text.clone(),
    };

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
    let outcome = agent
        .run_turn(&mut session, Message::user(prompt), tx)
        .await;
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

/// Skills the user can run as `/name`, for completion in the composer.
///
/// A separate call from [`list_skills`] rather than a filter in the UI: which
/// skills are user-invocable is the library's answer to give, and a composer
/// offering one the harness would then refuse is a dead end typed in full.
#[tauri::command]
pub async fn list_commands(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<SkillSummary>> {
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
    Ok(state.host.agents().await)
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
    Ok(path.display().to_string())
}

/// What the model is told to produce for the editor's Generate button.
///
/// Named fields rather than "write an agent file": the editor owns the format,
/// and a model asked for YAML frontmatter returns YAML that is *nearly* right
/// often enough to matter. JSON it can be checked against, and every field
/// lands in a box the user can correct.
const DRAFT_SYSTEM: &str = "\
You draft a sub-agent definition for a coding agent, and reply with JSON only — \
no prose, no markdown fence.

Fields:
- name: kebab-case, specific, e.g. \"migration-checker\"
- description: one sentence under 200 characters saying when to delegate here. \
This is the only text the caller sees when choosing, so describe the job.
- tools: an array chosen ONLY from the allowed list you are given, or null to \
inherit the caller's tools. Pick the narrowest set that can do the work.
- max_iterations: 1 to 50. Around 20 for work that only reads, 25 if it writes.
- prompt: the agent's system prompt. It shares none of the caller's context and \
cannot ask questions, so say what to do, what not to do, and what to report \
back. Several sentences.";

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
    request.system = Some(DRAFT_SYSTEM.into());

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
    proposal.max_iterations = drafted.max_iterations.unwrap_or(20).clamp(1, 50);
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
    Ok(Some(dir.display().to_string()))
}

#[tauri::command]
pub async fn set_skill_synthesis(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    state.host.set_skill_synthesis(enabled).await;
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
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub async fn set_agent_synthesis(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    state.host.set_agent_synthesis(enabled).await;
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
    Ok(())
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
