//! Application state.
//!
//! Thin by design: the harness itself lives in [`taurus_host::Host`], and this
//! adds only what a windowed, multi-session frontend needs on top — live
//! sessions, and the two maps that let an async decision be answered later by
//! a command from the webview.

use std::sync::Arc;

use dashmap::DashMap;
use tauri::AppHandle;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use taurus_agents::proposal::AgentProposal;
use taurus_core::Session;
use taurus_host::{Host, PermissionPromptFactory, SessionLog};
use taurus_skills::proposal::SkillProposal;
use taurus_tools::{PermissionDecision, PermissionPrompt};

use crate::bridge::{UiAgentProposalSink, UiPermissionPrompt, UiProposalSink};

/// One live conversation.
pub struct SessionEntry {
    pub session: Arc<Mutex<Session>>,
    pub provider_id: String,
    /// Cancels the in-flight turn. Replaced after each turn so a cancellation
    /// does not poison the next one.
    pub cancel: Arc<Mutex<CancellationToken>>,
    /// The transcript this conversation appends to. Its own lock rather than
    /// the session's, so a listing or a close does not wait behind a turn.
    pub log: Arc<Mutex<SessionLog>>,
}

/// Makes UI-backed permission prompts as the host rebuilds its engine.
struct UiPrompts {
    app: AppHandle,
    pending: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>>,
}

impl PermissionPromptFactory for UiPrompts {
    fn create(&self) -> Box<dyn PermissionPrompt> {
        Box::new(UiPermissionPrompt::new(
            self.app.clone(),
            self.pending.clone(),
        ))
    }
}

pub struct AppState {
    pub host: Host,
    pub sessions: DashMap<String, Arc<SessionEntry>>,
    pub pending_permissions: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>>,
    pub pending_proposals: Arc<DashMap<String, SkillProposal>>,
    pub pending_agent_proposals: Arc<DashMap<String, AgentProposal>>,
}

impl AppState {
    pub fn new(app: AppHandle) -> Self {
        let pending_permissions: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>> =
            Arc::new(DashMap::new());
        let pending_proposals: Arc<DashMap<String, SkillProposal>> = Arc::new(DashMap::new());
        let pending_agent_proposals: Arc<DashMap<String, AgentProposal>> = Arc::new(DashMap::new());

        let host = Host::new(
            Host::default_workspace(),
            Arc::new(UiPrompts {
                app: app.clone(),
                pending: pending_permissions.clone(),
            }),
            Arc::new(UiProposalSink::new(app.clone(), pending_proposals.clone())),
            Arc::new(UiAgentProposalSink::new(
                app,
                pending_agent_proposals.clone(),
            )),
        );

        Self {
            host,
            sessions: DashMap::new(),
            pending_permissions,
            pending_proposals,
            pending_agent_proposals,
        }
    }

    pub fn session(&self, id: &str) -> Result<Arc<SessionEntry>, String> {
        self.sessions
            .get(id)
            .map(|e| e.clone())
            .ok_or_else(|| format!("no session '{id}'"))
    }
}
