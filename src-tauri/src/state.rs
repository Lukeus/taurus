//! Application state.
//!
//! Thin by design: the harness itself lives in [`taurus_host::Host`], and this
//! adds only what a windowed, multi-session frontend needs on top — live
//! sessions, and the two maps that let an async decision be answered later by
//! a command from the webview.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use tauri::AppHandle;
use tokio::sync::{oneshot, watch, Mutex};
use tokio_util::sync::CancellationToken;

use taurus_agents::proposal::AgentProposal;
use taurus_core::telemetry::Traces;
use taurus_core::Session;
use taurus_host::{Host, PermissionPromptFactory, SessionLog, Switch};
use taurus_skills::proposal::SkillProposal;
use taurus_tools::{Answer, PermissionDecision, PermissionPrompt};

use crate::bridge::{UiAgentProposalSink, UiAsker, UiPermissionPrompt, UiProposalSink};
use crate::terminal::Terminals;

/// One live conversation.
pub struct SessionEntry {
    pub session: Arc<Mutex<Session>>,
    /// The backend serving this conversation *now*.
    ///
    /// Behind a lock rather than fixed at creation, because a conversation can
    /// move to another model or another backend without being a new
    /// conversation — see `switch_model`. Read at the start of every turn, so
    /// what a turn is sent to is whatever this said when it began.
    pub provider_id: Mutex<String>,
    /// The workspace this conversation belongs to, which is not always the one
    /// open.
    ///
    /// A conversation is bound to a folder in three ways at once: its
    /// transcript is written under that folder's key, its checkpoints are
    /// stored under it, and every file path it has ever mentioned describes
    /// that tree. None of them follow the window when someone opens another
    /// project, so this is what everything reading or continuing the
    /// conversation resolves against — and what a turn sent from the wrong
    /// folder is refused by.
    pub workspace: PathBuf,
    /// Cancels the in-flight turn. Replaced after each turn so a cancellation
    /// does not poison the next one.
    pub cancel: Arc<Mutex<CancellationToken>>,
    /// The transcript this conversation appends to. Its own lock rather than
    /// the session's, so a listing or a close does not wait behind a turn.
    pub log: Arc<Mutex<SessionLog>>,
    /// Where this conversation has changed model, oldest first.
    ///
    /// Held as well as written down, so that reopening a conversation that is
    /// still live redraws the same transcript a reopened-from-disk one would.
    /// Without it the two paths disagree: the file has the switches and the
    /// session in memory does not.
    pub switches: Mutex<Vec<Switch>>,
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
    /// Kept so any command can push state to the window without taking a handle
    /// as an argument.
    ///
    /// The alternative — an `AppHandle` parameter on every mutating command —
    /// makes announcing a change something each command opts into by changing
    /// its signature, which is exactly the kind of thing that gets left out of
    /// the next one somebody adds.
    pub app: AppHandle,
    pub host: Host,
    pub sessions: DashMap<String, Arc<SessionEntry>>,
    pub pending_permissions: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>>,
    /// Tool calls parked on a question card, keyed by call id.
    pub pending_questions: Arc<DashMap<String, oneshot::Sender<Vec<Answer>>>>,
    pub pending_proposals: Arc<DashMap<String, SkillProposal>>,
    pub pending_agent_proposals: Arc<DashMap<String, AgentProposal>>,
    /// Every shell open in the terminal dock.
    ///
    /// Here rather than beside the sessions because a terminal is not part of a
    /// conversation: it outlives every turn, belongs to the window, and is the
    /// one thing in this struct whose children keep running if nobody tidies
    /// them up. See [`Terminals::close_all`], and the call to it as the window
    /// goes away.
    pub terminals: Arc<Terminals>,
    /// The spans this process has finished, for the trace panel.
    ///
    /// A handle onto the ring the subscriber's recorder writes into, taken from
    /// the telemetry guard in `run` — not a second buffer. It has to come from
    /// there because the subscriber is installed before the window exists, and
    /// it can only be installed once.
    ///
    /// Process-wide rather than per session, which is what makes the panel's
    /// second tab possible: "is it always like this" is a question about the
    /// machine, and every conversation this window has run is the sample.
    pub traces: Traces,
    /// Cancels a **Build index** started from Settings.
    ///
    /// One, not one per session: the index belongs to the workspace rather than
    /// to any conversation, and there is one settings pane. Replaced at the
    /// start of each build, the same way a session's token is — reusing a
    /// cancelled one would stop the next build before it began.
    pub index_build: Mutex<CancellationToken>,
    /// Whether the first [`Host::reload`] has finished.
    ///
    /// The window paints before the filesystem has been read — skill discovery
    /// is deliberately kept off the setup path — so for the first moments the
    /// host holds an empty skill catalog. A status answered in that window
    /// reports zero skills, and the frontend takes that snapshot once and keeps
    /// it, so the rail reads `0` beside a drawer full of skills until some
    /// unrelated action happens to refresh it. Waiting here is the difference
    /// between a status that is slightly later and one that is durably wrong.
    ///
    /// Only the *first* load is gated, and only its local half — see
    /// `Host::reload_local`. A later reload replaces a populated catalog with
    /// another populated one, and blocking every status on it would put an MCP
    /// server's startup in front of the window; so would waiting here for the
    /// first load's MCP half, which is why `run` marks this between the two.
    loaded: watch::Sender<bool>,
}

/// How long a status waits for that first load before answering anyway.
///
/// Generous, because it is a backstop rather than the path: discovery reads a
/// handful of directories and finishes in milliseconds.
const FIRST_LOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

impl AppState {
    pub fn new(app: AppHandle, traces: Traces) -> Self {
        let pending_permissions: Arc<DashMap<String, oneshot::Sender<PermissionDecision>>> =
            Arc::new(DashMap::new());
        let pending_questions: Arc<DashMap<String, oneshot::Sender<Vec<Answer>>>> =
            Arc::new(DashMap::new());
        let pending_proposals: Arc<DashMap<String, SkillProposal>> = Arc::new(DashMap::new());
        let pending_agent_proposals: Arc<DashMap<String, AgentProposal>> = Arc::new(DashMap::new());

        let host = Host::new(
            Host::default_workspace(),
            Arc::new(UiPrompts {
                app: app.clone(),
                pending: pending_permissions.clone(),
            }),
            Arc::new(UiAsker::new(pending_questions.clone())),
            Arc::new(UiProposalSink::new(app.clone(), pending_proposals.clone())),
            Arc::new(UiAgentProposalSink::new(
                app.clone(),
                pending_agent_proposals.clone(),
            )),
        );

        Self {
            app,
            host,
            sessions: DashMap::new(),
            pending_permissions,
            pending_questions,
            pending_proposals,
            pending_agent_proposals,
            terminals: Arc::new(Terminals::default()),
            traces,
            index_build: Mutex::new(CancellationToken::new()),
            loaded: watch::channel(false).0,
        }
    }

    pub fn session(&self, id: &str) -> Result<Arc<SessionEntry>, String> {
        self.sessions
            .get(id)
            .map(|e| e.clone())
            .ok_or_else(|| format!("no session '{id}'"))
    }

    /// Marks the first reload done, releasing anything waiting on it.
    pub fn mark_loaded(&self) {
        let _ = self.loaded.send(true);
    }

    /// Waits for the first reload, so a status cannot answer from an empty
    /// catalog.
    ///
    /// Bounded, because a status that never answers is worse than one that
    /// undercounts: past the wait this degrades to reporting whatever has
    /// loaded so far, which is where it started.
    pub async fn loaded(&self) {
        let mut rx = self.loaded.subscribe();
        let _ = tokio::time::timeout(FIRST_LOAD_WAIT, async {
            loop {
                if *rx.borrow_and_update() {
                    return;
                }
                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
    }
}
