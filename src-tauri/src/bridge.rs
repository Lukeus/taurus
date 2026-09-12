//! Connects the harness's decision points to the user interface.
//!
//! Three things need a human: a permission prompt, which blocks the tool call
//! until answered; a skill proposal, which blocks nothing; and `ask_user`,
//! which blocks like the first and is delivered like neither.
//!
//! The odd one out is `ask_user`, and deliberately so. The other two emit a
//! Tauri event because the frontend has no other way to hear about them. Its
//! question card arrives on the turn's own event stream, as part of the tool
//! call being announced, which is what puts it in the transcript in the order
//! it was asked. So there is nothing to emit here — only somewhere for the call
//! to wait while the card is on screen.
//!
//! Alongside those decision points, this is also where the backend tells the
//! frontend that something it is showing has moved: [`EVENT_STATUS`] and
//! [`EVENT_SESSION`]. Those carry no question and block nothing. They exist
//! because the frontend used to find out by asking — after a turn, after a
//! click — which meant everything on screen was as old as the last thing the
//! user happened to do.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tracing::warn;

use taurus_agents::proposal::{AgentProposal, AgentProposalSink};
use taurus_core::{Session, TurnRecorder};
use taurus_host::{sessions, SessionLog};
use taurus_skills::proposal::{ProposalSink, SkillProposal};
use taurus_tools::{
    Answer, Asker, PermissionDecision, PermissionPrompt, PermissionRequest, Question,
};

pub const EVENT_PERMISSION_REQUEST: &str = "taurus://permission-request";
pub const EVENT_SKILL_PROPOSAL: &str = "taurus://skill-proposal";
pub const EVENT_AGENT_PROPOSAL: &str = "taurus://agent-proposal";

/// The whole of [`crate::commands::AppStatus`], whenever any of it moves.
///
/// The state carried rather than a nudge to go and fetch it: a "something
/// changed" ping costs a round trip to learn what, and the sender already has
/// the answer in hand.
pub const EVENT_STATUS: &str = "taurus://status";

/// One conversation's listing entry, when it appears or changes.
///
/// Singular on purpose. The frontend merges it into the list it already has,
/// so a turn ending costs one file read rather than a scan of every transcript
/// in the workspace.
pub const EVENT_SESSION: &str = "taurus://session";

/// The whole set of files one conversation has changed, when it is cut back.
///
/// A turn reports what it changes on the turn's own event stream, as it changes
/// them, so this exists for the one thing that moves the set the other way: a
/// rewind, which puts files back and drops the turns that touched them.
pub const EVENT_CHANGED: &str = "taurus://changed";

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

/// A request parked in one of the pending maps, taken out again however the
/// wait for it ends.
///
/// An answer takes it out, and so does a failed delivery. What neither covers is
/// the wait being dropped — Stop winning the race against the prompt, a turn
/// torn down — and an entry left behind then is a request that can still be
/// answered into nothing, in a map that only grows.
struct Parked<'a, T> {
    pending: &'a DashMap<String, T>,
    id: String,
}

impl<T> Drop for Parked<'_, T> {
    fn drop(&mut self) {
        self.pending.remove(&self.id);
    }
}

#[async_trait]
impl PermissionPrompt for UiPermissionPrompt {
    async fn request(&self, request: PermissionRequest) -> PermissionDecision {
        let id = request.id.clone();
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id.clone(), tx);
        let _parked = Parked {
            pending: &self.pending,
            id,
        };

        if let Err(e) = self.app.emit(EVENT_PERMISSION_REQUEST, &request) {
            // With no UI listening the call can never be approved, so deny
            // rather than block the turn forever.
            warn!(error = %e, "could not deliver permission request; denying");
            return PermissionDecision::Deny;
        }

        // Sender dropped: the window closed or the session was torn down.
        rx.await.unwrap_or(PermissionDecision::Deny)
    }
}

/// Where a blocked `ask_user` call waits for the card to be answered.
///
/// Keyed by the tool call's id, which is the same id the card in the transcript
/// was drawn from — so a click on it can find the call it belongs to.
/// `answer_questions` completes it.
pub struct UiAsker {
    pending: Arc<DashMap<String, oneshot::Sender<Vec<Answer>>>>,
}

impl UiAsker {
    pub fn new(pending: Arc<DashMap<String, oneshot::Sender<Vec<Answer>>>>) -> Self {
        Self { pending }
    }
}

#[async_trait]
impl Asker for UiAsker {
    async fn ask(&self, id: &str, _questions: &[Question]) -> Option<Vec<Answer>> {
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id.to_string(), tx);
        let _parked = Parked {
            pending: &self.pending,
            id: id.to_string(),
        };

        // Sender dropped: the window closed, or the session was torn down with a
        // card still on screen. Answering nothing lets the turn finish on its
        // own judgement instead of holding the loop open on a card nobody can
        // reach.
        rx.await.ok()
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

/// Writes the conversation on screen down as it happens, and says when it first
/// lands.
///
/// The agent loop records a turn once per tool round trip and once when it
/// ends, so wiring this in is most of what makes the app's state live: the
/// transcript exists from the moment the question is asked rather than from the
/// moment it is answered.
///
/// The announcement is made exactly once per transcript — when the header is
/// written — because that is the only round that changes what a listing would
/// show. Every later round appends messages nobody is reading off disk; the
/// view of them is the event stream the turn is already sending.
pub struct UiSessionLog {
    app: AppHandle,
    /// Shared with the command that owns the session, which still records the
    /// finished turn under it. One lock, so the two cannot interleave halfway
    /// through a write.
    log: Arc<tokio::sync::Mutex<SessionLog>>,
    id: String,
}

impl UiSessionLog {
    pub fn new(app: AppHandle, log: Arc<tokio::sync::Mutex<SessionLog>>, id: String) -> Self {
        Self { app, log, id }
    }
}

#[async_trait]
impl TurnRecorder for UiSessionLog {
    async fn record(&self, session: &Session) {
        // Held by an owned guard so it can go with the write to a blocking
        // thread. The write is disk I/O once per tool round, and the runtime it
        // would otherwise run on is the one carrying the stream.
        let mut log = self.log.clone().lock_owned().await;
        let pending = log.unrecorded(session);
        let id = self.id.clone();
        let announced = tokio::task::spawn_blocking(move || {
            // Read back rather than assembled here, so what the rail shows is
            // what a later listing will show — including how the title was
            // derived and shortened, which is the transcript layer's rule and
            // not this one's.
            log.record_unrecorded(pending)
                .then(|| sessions::meta(&id))
                .flatten()
        })
        .await
        .ok()
        .flatten();
        let Some(meta) = announced else {
            return;
        };
        if let Err(e) = self.app.emit(EVENT_SESSION, &meta) {
            // The conversation is on disk regardless; the rail catches up on
            // the next thing that lists it.
            warn!(error = %e, "could not announce a new conversation");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_frontend_listens_for_the_names_this_emits() {
        // Each name is spelled twice, here and in `src/lib/api.ts`. A rename on
        // one side compiles on both and fails only when the app runs: the
        // listener waits for an event nothing sends.
        let api = include_str!("../../src/lib/api.ts");
        for (name, value) in [
            ("EVENT_PERMISSION_REQUEST", EVENT_PERMISSION_REQUEST),
            ("EVENT_SKILL_PROPOSAL", EVENT_SKILL_PROPOSAL),
            ("EVENT_AGENT_PROPOSAL", EVENT_AGENT_PROPOSAL),
            ("EVENT_STATUS", EVENT_STATUS),
            ("EVENT_SESSION", EVENT_SESSION),
            ("EVENT_CHANGED", EVENT_CHANGED),
        ] {
            let line = format!("export const {name} = \"{value}\";");
            assert!(api.contains(&line), "src/lib/api.ts has no `{line}`");
        }
    }

    use super::*;

    #[tokio::test]
    async fn a_question_whose_wait_is_dropped_leaves_nothing_parked() {
        // Stop races the wait and wins, so the wait is dropped rather than
        // answered. An entry left behind is a card that can still be answered
        // into nothing, in a map that only grows. The permission prompt parks
        // its requests through the same guard.
        let pending: Arc<DashMap<String, oneshot::Sender<Vec<Answer>>>> = Arc::new(DashMap::new());
        let asker = Arc::new(UiAsker::new(pending.clone()));
        let waiting = {
            let asker = asker.clone();
            tokio::spawn(async move { asker.ask("call-1", &[]).await })
        };
        for _ in 0..100 {
            if !pending.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            pending.contains_key("call-1"),
            "the question was never parked"
        );

        waiting.abort();
        let _ = waiting.await;
        assert!(
            pending.is_empty(),
            "a dropped wait left its question parked"
        );
    }
}
