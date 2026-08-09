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
use taurus_tools::PermissionDecision;

use taurus_host::{ProviderConfig, Settings};

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
    pub skill_problems: Vec<String>,
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
        skill_problems: state.host.problems().await,
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
    let id = session.id.clone();
    state.sessions.insert(
        id.clone(),
        Arc::new(SessionEntry {
            session: Arc::new(Mutex::new(session)),
            provider_id: provider_id.clone(),
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
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
    let agent = state.host.build_agent(provider, &model, cancel).await;

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

#[tauri::command]
pub async fn list_permission_rules(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<String>> {
    Ok(state.host.permissions().await.allowed_rules().await)
}

#[tauri::command]
pub async fn revoke_permission_rule(
    state: State<'_, Arc<AppState>>,
    rule: String,
) -> CmdResult<()> {
    state.host.permissions().await.revoke(&rule).await;
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
