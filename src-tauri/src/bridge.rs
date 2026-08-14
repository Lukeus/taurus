//! Connects the harness's decision points to the user interface.
//!
//! Two things in `taurus-core` need a human: a permission prompt, which blocks
//! the tool call until answered, and a skill proposal, which does not block
//! anything. Both are traits there and become Tauri events here.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tracing::warn;

use taurus_agents::proposal::{AgentProposal, AgentProposalSink};
use taurus_skills::proposal::{ProposalSink, SkillProposal};
use taurus_tools::{PermissionDecision, PermissionPrompt, PermissionRequest};

pub const EVENT_PERMISSION_REQUEST: &str = "taurus://permission-request";
pub const EVENT_SKILL_PROPOSAL: &str = "taurus://skill-proposal";
pub const EVENT_AGENT_PROPOSAL: &str = "taurus://agent-proposal";

/// Permission prompt backed by the UI.
///
/// A request is parked here keyed by its id, the UI is notified, and the tool
/// call awaits the answer. `respond_permission` completes it.
pub struct UiPermissionPrompt {
    app: AppHandle,
    pending: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>>,
}

impl UiPermissionPrompt {
    pub fn new(
        app: AppHandle,
        pending: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>>,
    ) -> Self {
        Self { app, pending }
    }
}

#[async_trait]
impl PermissionPrompt for UiPermissionPrompt {
    async fn request(&self, request: PermissionRequest) -> PermissionDecision {
        let id = request.id.clone();
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id.clone(), tx);

        if let Err(e) = self.app.emit(EVENT_PERMISSION_REQUEST, &request) {
            // With no UI listening the call can never be approved, so deny
            // rather than block the turn forever.
            warn!(error = %e, "could not deliver permission request; denying");
            self.pending.remove(&id);
            return PermissionDecision::Deny;
        }

        match rx.await {
            Ok(decision) => decision,
            // Sender dropped: the window closed or the session was torn down.
            Err(_) => {
                self.pending.remove(&id);
                PermissionDecision::Deny
            }
        }
    }
}

/// Skill proposals surfaced as review cards.
///
/// Submission returns immediately: the agent keeps working while the proposal
/// waits for a human. Approval is handled by `respond_skill_proposal`.
pub struct UiProposalSink {
    app: AppHandle,
    pending: Arc<DashMap<String, SkillProposal>>,
}

impl UiProposalSink {
    pub fn new(app: AppHandle, pending: Arc<DashMap<String, SkillProposal>>) -> Self {
        Self { app, pending }
    }
}

#[async_trait]
impl ProposalSink for UiProposalSink {
    async fn submit(&self, proposal: SkillProposal) {
        self.pending.insert(proposal.id.clone(), proposal.clone());
        if let Err(e) = self.app.emit(EVENT_SKILL_PROPOSAL, &proposal) {
            warn!(error = %e, "could not deliver skill proposal");
        }
    }
}

/// The same, for proposed sub-agents. Approval is handled by
/// `respond_agent_proposal`.
pub struct UiAgentProposalSink {
    app: AppHandle,
    pending: Arc<DashMap<String, AgentProposal>>,
}

impl UiAgentProposalSink {
    pub fn new(app: AppHandle, pending: Arc<DashMap<String, AgentProposal>>) -> Self {
        Self { app, pending }
    }
}

#[async_trait]
impl AgentProposalSink for UiAgentProposalSink {
    async fn submit(&self, proposal: AgentProposal) {
        self.pending.insert(proposal.id.clone(), proposal.clone());
        if let Err(e) = self.app.emit(EVENT_AGENT_PROPOSAL, &proposal) {
            warn!(error = %e, "could not deliver agent proposal");
        }
    }
}
