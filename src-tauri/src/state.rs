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

use taurus_core::Session;
use taurus_host::{Host, PermissionPromptFactory};
use taurus_skills::proposal::SkillProposal;
use taurus_tools::{PermissionDecision, PermissionPrompt};

use crate::bridge::{UiPermissionPrompt, UiProposalSink};

/// One live conversation.
pub struct SessionEntry {
    pub session: Arc<Mutex<Session>>,
    pub provider_id: String,
    /// Cancels the in-flight turn. Replaced after each turn so a cancellation
    /// does not poison the next one.
    pub cancel: Arc<Mutex<CancellationToken>>,
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
}

impl AppState {
    pub fn new(app: AppHandle) -> Self {
        let pending_permissions: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>> =
            Arc::new(DashMap::new());
        let pending_proposals: Arc<DashMap<String, SkillProposal>> = Arc::new(DashMap::new());

        let host = Host::new(
            Host::default_workspace(),
            Arc::new(UiPrompts {
                app: app.clone(),
                pending: pending_permissions.clone(),
            }),
            Arc::new(UiProposalSink::new(app, pending_proposals.clone())),
        );

        Self {
            host,
            sessions: DashMap::new(),
            pending_permissions,
            pending_proposals,
        }
    }

    pub fn session(&self, id: &str) -> Result<Arc<SessionEntry>, String> {
        self.sessions
            .get(id)
            .map(|e| e.clone())
            .ok_or_else(|| format!("no session '{id}'"))
    }
}
