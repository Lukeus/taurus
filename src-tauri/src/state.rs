//! Application state.
//!
//! Thin by design: the harness itself lives in [`taurus_host::Host`], and this
//! adds only what a windowed, multi-session frontend needs on top — live
//! sessions, and the two maps that let an async decision be answered later by
//! a command from the webview.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tauri::AppHandle;
use tokio::sync::{oneshot, watch, Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

use taurus_agents::proposal::AgentProposal;
use taurus_core::telemetry::Traces;
use taurus_core::Session;
use taurus_host::{Host, PermissionPromptFactory, SessionLog, Switch};
use taurus_skills::proposal::SkillProposal;
use taurus_tools::{Answer, PermissionDecision, PermissionPrompt};

use crate::bridge::{UiAgentProposalSink, UiAsker, UiPermissionPrompt, UiProposalSink};
use crate::live::Live;
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
    /// The model this conversation is on, mirrored out of the session.
    ///
    /// The session holds it too, but a turn holds the session's lock for its
    /// whole run, and asking which model a conversation is on — the first
    /// thing a turn review does — must not wait minutes for the answer.
    /// Written wherever the session's own copy is: at creation, on resume, and
    /// by `switch_model`.
    pub model: Mutex<String>,
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
    /// The turn running in this conversation, and every view watching it.
    ///
    /// `Some` for exactly as long as a turn runs. It is here rather than inside
    /// the call that started the turn because a turn outlives the view of it:
    /// the window can reload, or move to another conversation, and find its way
    /// back to this. See [`crate::live`].
    pub live: Mutex<Option<Arc<Live>>>,
    /// Whether this conversation runs without anybody to answer it.
    ///
    /// Off by default and never persisted: it is a statement about the next few
    /// hours rather than about the project, and a conversation reopened
    /// tomorrow should be asked again. Shared with the turn rather than read
    /// per turn, because the moment it is set is usually a moment when a turn
    /// is already running — somebody on their way out.
    ///
    /// It widens nothing. What a standing grant already permits never reaches a
    /// prompt, so this only decides what becomes of the calls that would have
    /// been asked about, and the answer it gives is no. See
    /// [`taurus_tools::Asking::unattended`].
    pub unattended: Arc<AtomicBool>,
    /// Set when the window has let go of this conversation while a turn was
    /// still running.
    ///
    /// Closing a conversation releases the live copy of it — every message and
    /// every image, otherwise held for the life of the process. A conversation
    /// mid-turn is still using all of it, so the release waits for the turn and
    /// this is what remembers that it was asked for. See `close_session`.
    pub released: AtomicBool,
    /// Where this conversation has changed model, oldest first.
    ///
    /// Held as well as written down, so that reopening a conversation that is
    /// still live redraws the same transcript a reopened-from-disk one would.
    /// Without it the two paths disagree: the file has the switches and the
    /// session in memory does not.
    pub switches: Mutex<Vec<Switch>>,
}

/// What letting go of a conversation means, asked at the moment of letting go.
#[derive(Debug, PartialEq, Eq)]
pub enum Release {
    /// Nothing is using it: drop the entry and everything keyed by it.
    Now,
    /// A turn is: it is still using the session, the transcript and the plan
    /// board, and it is noted down to be released when it ends.
    WhenTheTurnEnds,
}

impl SessionEntry {
    /// Lets go of the live copy of this conversation, or arranges to.
    ///
    /// The whole of the rule that makes a long turn survivable: leaving is not
    /// stopping. Until this existed, closing a conversation cancelled the turn
    /// in it — and closing is what happens on every switch between
    /// conversations.
    pub async fn release(&self) -> Release {
        if self.live.lock().await.is_some() {
            self.released.store(true, Ordering::Relaxed);
            return Release::WhenTheTurnEnds;
        }
        Release::Now
    }

    /// Whether a release was asked for while a turn was running.
    ///
    /// Read once the turn has ended, by whoever ran it.
    pub fn was_released(&self) -> bool {
        self.released.load(Ordering::Relaxed)
    }

    /// Takes back a release, for a conversation reopened before its turn ended.
    ///
    /// Coming back to a conversation is the opposite of letting go of it, and
    /// without this the turn ending would reap an entry the window is using.
    pub fn keep(&self) {
        self.released.store(false, Ordering::Relaxed);
    }

    /// The conversation, if no turn is running in it.
    ///
    /// A turn holds the session's lock for its whole run, which is minutes for
    /// a long one. What must not happen underneath a turn — a rewind, a commit,
    /// a delete, a change of model — asks through this and is refused at once,
    /// rather than left waiting behind the turn or run alongside it. `refusal`
    /// finishes the sentence with what to do instead.
    ///
    /// A caller that only needs the answer lets the guard go at once; one that
    /// changes the session keeps it for as long as the change takes.
    pub fn idle(&self, refusal: &str) -> Result<MutexGuard<'_, Session>, String> {
        self.session
            .try_lock()
            .map_err(|_| format!("this conversation is mid-turn; {refusal}"))
    }
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
    /// The model calls the window starts outside a turn — a turn review, an
    /// agent draft — so the pane that started one can stop it.
    pub stoppable: Stoppable,
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

/// Jobs the window starts and can stop, each filed under a key.
///
/// A key per job rather than one token for all of them, the way the index
/// build has one: a review of turn 2 and a review of turn 5 can run at once,
/// and stopping one must not stop the other. A job is filed away again when it
/// finishes, so this holds only what is running.
#[derive(Default)]
pub struct Stoppable {
    running: DashMap<String, (u64, CancellationToken)>,
    tickets: std::sync::atomic::AtomicU64,
}

/// A job that has started. Dropping it files the job away.
pub struct Running<'a> {
    /// What the job watches for Stop.
    pub cancel: CancellationToken,
    slot: &'a Stoppable,
    key: String,
    ticket: u64,
}

impl Stoppable {
    /// A fresh token for a job starting under `key`.
    ///
    /// A job already running under the same key is stopped first: the same
    /// thing asked for twice is one job, and the first answer would arrive at a
    /// pane that is waiting for the second.
    pub fn start(&self, key: impl Into<String>) -> Running<'_> {
        let key = key.into();
        let ticket = self
            .tickets
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cancel = CancellationToken::new();
        if let Some((_, previous)) = self.running.insert(key.clone(), (ticket, cancel.clone())) {
            previous.cancel();
        }
        Running {
            cancel,
            slot: self,
            key,
            ticket,
        }
    }

    /// Stops the job running under `key`. Safe to call when none is: a pane
    /// calls this on its way out without knowing whether its job finished.
    pub fn stop(&self, key: &str) {
        if let Some((_, (_, cancel))) = self.running.remove(key) {
            cancel.cancel();
        }
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        // By ticket, so a job that was replaced leaves its replacement filed.
        self.slot
            .running
            .remove_if(&self.key, |_, (ticket, _)| *ticket == self.ticket);
    }
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
            stoppable: Stoppable::default(),
            loaded: watch::channel(false).0,
        }
    }

    pub fn session(&self, id: &str) -> Result<Arc<SessionEntry>, String> {
        self.sessions
            .get(id)
            .map(|e| e.clone())
            .ok_or_else(|| format!("no session '{id}'"))
    }

    /// Stops every turn in flight and withdraws every question they were
    /// waiting on.
    ///
    /// For a page that has been replaced: a reload, a dev server restarting.
    /// The frontend that started those turns is gone, and so is every dialog it
    /// was showing. A turn parked on one would wait forever holding its
    /// session's lock, and delete, rewind, and commit would refuse that
    /// conversation until the app restarted. Clearing the maps drops each
    /// parked sender, so a prompt the cancel has not reached yet is answered
    /// with a denial rather than left.
    pub async fn abandon_turns(&self) {
        // Collected first, so no map guard is held across an await.
        let entries: Vec<Arc<SessionEntry>> =
            self.sessions.iter().map(|e| e.value().clone()).collect();
        for entry in entries {
            entry.cancel.lock().await.cancel();
        }
        self.pending_permissions.clear();
        self.pending_questions.clear();
    }

    /// Marks the first reload done, releasing anything waiting on it.
    pub fn mark_loaded(&self) {
        finish_load(&self.loaded);
    }

    /// Waits for the first reload, so a status cannot answer from an empty
    /// catalog.
    ///
    /// Bounded, because a status that never answers is worse than one that
    /// undercounts: past the wait this degrades to reporting whatever has
    /// loaded so far, which is where it started.
    pub async fn loaded(&self) {
        wait_for_load(&self.loaded, FIRST_LOAD_WAIT).await;
    }
}

/// Records that the first load is done, whether or not anyone is waiting yet.
///
/// `send_replace` rather than `send`, and that is the whole of the fix this
/// exists for. `send` refuses when nothing is subscribed — and refuses by
/// dropping the value, not by keeping it for later. The load finishing before
/// the window's first `get_status` arrived is the ordinary case, so the flag
/// was ordinarily lost: every status after it subscribed to a `false` that
/// would never change, and waited out the whole of `FIRST_LOAD_WAIT`. That was
/// ten seconds on every launch and every page reload, and it looked like a
/// slow load rather than a missed signal.
fn finish_load(loaded: &watch::Sender<bool>) {
    loaded.send_replace(true);
}

/// Waits until [`finish_load`] has run, or `limit` passes.
async fn wait_for_load(loaded: &watch::Sender<bool>, limit: std::time::Duration) {
    let mut rx = loaded.subscribe();
    let _ = tokio::time::timeout(limit, async {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_load_that_finished_before_anyone_asked_is_not_forgotten() {
        // The ordinary order: the reload is done before the window's first
        // status request arrives, so nothing is subscribed when it finishes.
        let (loaded, _) = watch::channel(false);
        finish_load(&loaded);

        let started = std::time::Instant::now();
        wait_for_load(&loaded, std::time::Duration::from_secs(5)).await;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "waited {:?} for a load that had already finished",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_status_asked_for_mid_load_is_answered_when_it_finishes() {
        let (loaded, _) = watch::channel(false);
        let loaded = Arc::new(loaded);
        let waiting = tokio::spawn({
            let loaded = loaded.clone();
            async move {
                let started = std::time::Instant::now();
                wait_for_load(&loaded, std::time::Duration::from_secs(5)).await;
                started.elapsed()
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        finish_load(&loaded);
        assert!(waiting.await.unwrap() < std::time::Duration::from_secs(1));
    }

    #[tokio::test]
    async fn a_load_that_never_finishes_gives_up_at_the_limit() {
        let (loaded, _) = watch::channel(false);
        let started = std::time::Instant::now();
        wait_for_load(&loaded, std::time::Duration::from_millis(100)).await;
        assert!(started.elapsed() >= std::time::Duration::from_millis(100));
    }

    #[test]
    fn stopping_a_job_cancels_its_token_and_no_other() {
        let jobs = Stoppable::default();
        let two = jobs.start("review/s1/2");
        let five = jobs.start("review/s1/5");

        jobs.stop("review/s1/2");

        assert!(two.cancel.is_cancelled());
        assert!(
            !five.cancel.is_cancelled(),
            "stopping one review stopped another"
        );
    }

    #[test]
    fn the_same_job_asked_for_twice_stops_the_first() {
        let jobs = Stoppable::default();
        let first = jobs.start("agent-draft");
        let second = jobs.start("agent-draft");
        assert!(first.cancel.is_cancelled());
        assert!(!second.cancel.is_cancelled());

        // The first one finishing must not file the second away with it, or
        // Stop would have nothing left to reach.
        drop(first);
        jobs.stop("agent-draft");
        assert!(second.cancel.is_cancelled());
    }

    #[test]
    fn a_finished_job_is_forgotten_and_stopping_it_is_harmless() {
        let jobs = Stoppable::default();
        let done = jobs.start("review/s1/1");
        let token = done.cancel.clone();
        drop(done);

        assert!(jobs.running.is_empty(), "a finished job is still filed");
        jobs.stop("review/s1/1");
        assert!(!token.is_cancelled());
    }
}
