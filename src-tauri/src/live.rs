//! A turn that outlives the call that started it.
//!
//! A turn used to stream to exactly one place: the IPC channel `send_message`
//! was handed. That made the call the owner of the turn, which is wrong in both
//! directions. A window that reloaded could never find its way back to a turn
//! still running, and a conversation switched away from was canceled outright,
//! because letting go of the view was the only way the app had to let go of the
//! work.
//!
//! So the stream belongs to the conversation instead. [`Live`] exists for
//! exactly as long as a turn does, holds every view watching it, and
//! `send_message`'s own channel is one of those views rather than the owner.
//!
//! # Arriving late
//!
//! The transcript is written once per tool round trip and once at the end (see
//! [`taurus_core::agent`]), so a view that opens the conversation mid-turn gets
//! everything up to the last completed round from disk — and nothing of the
//! round in progress, which is the part being watched.
//!
//! That is what [`Replay`] holds, and why it is cleared on every
//! `IterationStarted`: what it keeps is exactly what disk does not have.
//!
//! ```text
//! the transcript on disk  +  the replay  =  the turn as it stands
//! ```
//!
//! Keeping one round rather than one turn is also what bounds it, without a
//! rule about when to forget. It is capped anyway, because one round of a
//! reasoning model is not small, and a view that arrives after the cap has
//! bitten is told how much of the round it is missing rather than left to
//! begin mid-sentence.
//!
//! # Delegates
//!
//! A delegate's run is the exception to "disk has the rounds that finished".
//! The parent's transcript keeps one call and its report, never the child's
//! steps, and a background delegate goes on across rounds of the parent. So
//! everything about delegations is kept apart, in `agents`, for the whole
//! turn: a window that opens mid-turn can draw every agent in it, not only
//! the ones started this round. It's sent after the round, so a card the
//! round is about to draw exists before what's said about it arrives.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(test)]
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::ipc::Channel;
use tokio::sync::Mutex;

use taurus_core::UiEvent;

/// Largest replay held for the round in progress.
///
/// The same size the background-command buffer uses, for the same reason: it
/// is enough for anything a person would sit and read, and past that what is
/// being kept is a log rather than a view. Deltas are merged as they arrive
/// (see [`Replay::push`]), so this is measured against a round's *text* rather
/// than against the thousands of events it arrived in.
const MAX_REPLAY_BYTES: usize = 256 * 1024;

/// What one event costs to hold, beyond its own payload.
///
/// A flat number rather than a measurement. The point of the cap is to bound
/// what a round can hold, and a round is made large by what is in one or two
/// events rather than by the count of small ones.
const EVENT_OVERHEAD: usize = 64;

/// Largest record of delegate activity held for the whole turn. See the
/// module's "Delegates". Larger than a round's, because it outlives rounds
/// and holds several agents; still a cap, because a delegate is a model too.
const MAX_AGENTS_BYTES: usize = 512 * 1024;

/// Charged for a call that carries something to draw in place of its row — a
/// query result, a plan board.
///
/// Measuring one means serializing it, on the path every event takes. The
/// payload is bounded where it is produced (a query answers thirty rows), so a
/// flat charge is both cheap and close enough for a cap.
const VIEW_WEIGHT: usize = 4 * 1024;

/// One turn, and everything watching it.
pub struct Live {
    /// The views and the replay under one lock, because attaching is
    /// "everything so far, then everything after" and the two halves must not
    /// be separated by an event arriving between them.
    fan: Mutex<Fan>,
    /// Unix seconds. Wall clock rather than an `Instant`, because the window
    /// has to be able to say how long the turn has been running and an
    /// `Instant` does not cross the IPC.
    started: u64,
    /// The round it is on, mirrored off the stream so it can be read without
    /// waiting for whatever is publishing.
    iteration: AtomicU32,
}

#[derive(Default)]
struct Fan {
    views: Vec<Channel<UiEvent>>,
    replay: Replay,
    /// Every delegation's events, for the whole turn. See the module's
    /// "Delegates".
    agents: Replay,
}

impl Live {
    pub fn new() -> Self {
        Self {
            fan: Mutex::new(Fan::default()),
            started: now(),
            iteration: AtomicU32::new(0),
        }
    }

    /// Unix seconds, for the window to count up from.
    pub fn started(&self) -> u64 {
        self.started
    }

    /// The round in progress. Zero before the first one has begun.
    pub fn iteration(&self) -> u32 {
        self.iteration.load(Ordering::Relaxed)
    }

    /// Hands one event to every view, and keeps it for a view yet to arrive.
    ///
    /// A view whose channel has gone — a reloaded webview, a closed window — is
    /// dropped here rather than waited for. That is the difference this whole
    /// module exists for: nobody watching is not a reason to stop working.
    pub async fn publish(&self, event: UiEvent) {
        let mut fan = self.fan.lock().await;
        if let UiEvent::IterationStarted { iteration } = &event {
            self.iteration.store(*iteration, Ordering::Relaxed);
            // The round the transcript is about to be missing is this one.
            fan.replay.clear();
        }
        if about_a_delegate(&event) {
            fan.agents.push_capped(event.clone(), MAX_AGENTS_BYTES);
        } else {
            fan.replay.push(event.clone());
        }
        fan.views.retain(|view| view.send(event.clone()).is_ok());
    }

    /// Adds a view, catching it up on the round in progress first.
    ///
    /// Returns how many events of that round are no longer held, which is
    /// always zero except on a round that overran the cap. The caller reports
    /// it; a view that silently begins mid-sentence is the one outcome worth
    /// avoiding here.
    pub async fn attach(&self, view: Channel<UiEvent>) -> usize {
        let mut fan = self.fan.lock().await;
        // The round first, then the delegates: a card drawn by this round has
        // to exist before what's said about its delegate lands on it.
        for event in fan.replay.events.iter().chain(&fan.agents.events) {
            // A view that cannot take the catch-up cannot take what follows
            // either, so it is never registered.
            if view.send(event.clone()).is_err() {
                return 0;
            }
        }
        let dropped = fan.replay.dropped + fan.agents.dropped;
        fan.views.push(view);
        dropped
    }

    /// Lets go of every view, for a page that has been replaced.
    ///
    /// A channel into a page that has gone doesn't always refuse a send, so
    /// `publish` can't be relied on to notice. The replay is kept: it's what the
    /// new page catches up from when it attaches.
    pub async fn forget_views(&self) {
        self.fan.lock().await.views.clear();
    }
}

/// The round in progress, for a view that has not arrived yet.
#[derive(Default)]
struct Replay {
    events: VecDeque<UiEvent>,
    /// What `events` weighs, kept beside it so a push is not a walk.
    bytes: usize,
    /// Events of this round dropped to stay under the cap.
    dropped: usize,
}

impl Replay {
    fn clear(&mut self) {
        self.events.clear();
        self.bytes = 0;
        self.dropped = 0;
    }

    /// Keeps one event, merging it with the last where that loses nothing.
    ///
    /// Consecutive deltas of one kind are one delta: that is what the fold at
    /// the other end does with them, and keeping them apart would spend the
    /// whole cap on the per-event overhead of a model that streams in small
    /// pieces.
    fn push(&mut self, event: UiEvent) {
        self.push_capped(event, MAX_REPLAY_BYTES);
    }

    fn push_capped(&mut self, event: UiEvent, cap: usize) {
        use taurus_tools::DelegateStep::{Text, Thinking};
        match (&event, self.events.back_mut()) {
            (UiEvent::TextDelta { text }, Some(UiEvent::TextDelta { text: held }))
            | (UiEvent::ThinkingDelta { text }, Some(UiEvent::ThinkingDelta { text: held })) => {
                held.push_str(text);
                self.bytes += text.len();
            }
            // A delegate streams as its parent does, and only one delegate's
            // run of a kind is merged: concurrent delegates interleave.
            (
                UiEvent::DelegateActivity {
                    id,
                    step: Text { text },
                },
                Some(UiEvent::DelegateActivity {
                    id: held_id,
                    step: Text { text: held },
                }),
            )
            | (
                UiEvent::DelegateActivity {
                    id,
                    step: Thinking { text },
                },
                Some(UiEvent::DelegateActivity {
                    id: held_id,
                    step: Thinking { text: held },
                }),
            ) if id == held_id => {
                held.push_str(text);
                self.bytes += text.len();
            }
            _ => {
                self.bytes += weight(&event);
                self.events.push_back(event);
            }
        }
        while self.bytes > cap {
            let Some(gone) = self.events.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(weight(&gone));
            self.dropped += 1;
        }
    }
}

/// Roughly what one event costs to hold.
///
/// Only the payloads that can be large are counted; everything else is the flat
/// overhead. See [`EVENT_OVERHEAD`] and [`VIEW_WEIGHT`].
fn weight(event: &UiEvent) -> usize {
    let payload = match event {
        UiEvent::TextDelta { text } | UiEvent::ThinkingDelta { text } => text.len(),
        UiEvent::ToolCallStarted { preview, view, .. } => {
            preview.len() + view.as_ref().map_or(0, |_| VIEW_WEIGHT)
        }
        UiEvent::ToolCallFinished { output, images, .. } => {
            output.len() + images.iter().map(|image| image.data.len()).sum::<usize>()
        }
        UiEvent::ToolProgress { label, .. } => label.len(),
        UiEvent::DelegateActivity { step, .. } => {
            use taurus_tools::DelegateStep as Step;
            match step {
                Step::Text { text } | Step::Thinking { text } => text.len(),
                Step::ToolStarted { preview, .. } => preview.len(),
                Step::Retrying { reason, .. } => reason.len(),
                Step::Error { message } => message.len(),
                _ => 0,
            }
        }
        UiEvent::DelegateReport { report, .. } => report.summary.len(),
        UiEvent::Retrying { reason, .. } => reason.len(),
        UiEvent::FilesChanged { paths } => paths.iter().map(String::len).sum(),
        UiEvent::Error { message } => message.len(),
        _ => 0,
    };
    EVENT_OVERHEAD + payload
}

/// Whether an event is about a delegation rather than the round it's in.
///
/// A delegation's steps, and the three events that tie a card to its child:
/// where the transcript is, that it went to the background, and its report.
/// A background delegate's arrive rounds after its card was drawn.
fn about_a_delegate(event: &UiEvent) -> bool {
    matches!(
        event,
        UiEvent::DelegateActivity { .. }
            | UiEvent::ToolTranscript { .. }
            | UiEvent::ToolDetached { .. }
            | UiEvent::DelegateReport { .. }
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    /// A view, and what it has been shown.
    ///
    /// A real `Channel`, because what is being tested is the thing `Live` does
    /// with one: it hands events to it and drops it when it will not take them.
    /// `send` calls the handler directly, so nothing here needs a webview.
    #[derive(Clone, Default)]
    struct View(Arc<StdMutex<Vec<String>>>);

    impl View {
        /// A channel that takes everything.
        fn open(&self) -> Channel<UiEvent> {
            let seen = self.0.clone();
            Channel::new(move |body| {
                seen.lock().unwrap().push(format!("{body:?}"));
                Ok(())
            })
        }

        /// A channel that is gone — a reloaded webview, a closed window.
        fn shut() -> Channel<UiEvent> {
            Channel::new(|_| Err(tauri::Error::WebviewNotFound))
        }

        fn seen(&self) -> usize {
            self.0.lock().unwrap().len()
        }

        fn said(&self) -> String {
            self.0.lock().unwrap().join(" ")
        }
    }

    #[tokio::test]
    async fn every_view_sees_every_event() {
        let live = Live::new();
        let one = View::default();
        let two = View::default();
        live.attach(one.open()).await;
        live.attach(two.open()).await;

        live.publish(text("hello")).await;

        assert_eq!(one.seen(), 1);
        assert_eq!(two.seen(), 1);
    }

    /// The whole point of the module. A window that has gone away used to stop
    /// the turn — first by breaking the one forwarder it had, later by
    /// cancelling outright. Now it is dropped and the work carries on.
    #[tokio::test]
    async fn a_view_that_has_gone_is_dropped_and_the_others_carry_on() {
        let live = Live::new();
        let staying = View::default();
        live.attach(View::shut()).await;
        live.attach(staying.open()).await;

        live.publish(text("first")).await;
        live.publish(text("second")).await;

        assert_eq!(staying.seen(), 2);
        // Asked for by the send that failed, not by a later sweep: a dead view
        // must not be tried again on every event for the rest of the turn.
        assert_eq!(live.fan.lock().await.views.len(), 1);
    }

    /// What a window that opens mid-turn is missing is exactly the round in
    /// progress, because the transcript is written at the end of each round.
    #[tokio::test]
    async fn a_view_arriving_mid_round_is_caught_up_on_that_round() {
        let live = Live::new();
        live.publish(UiEvent::IterationStarted { iteration: 1 })
            .await;
        live.publish(text("the first round")).await;
        live.publish(UiEvent::IterationStarted { iteration: 2 })
            .await;
        live.publish(text("the second ")).await;

        let late = View::default();
        let dropped = live.attach(late.open()).await;
        live.publish(text("round")).await;

        assert_eq!(dropped, 0);
        let said = late.said();
        assert!(said.contains("the second "), "{said}");
        assert!(
            said.contains("round"),
            "the rest of the round follows: {said}"
        );
        assert!(
            !said.contains("the first round"),
            "the transcript on disk has the rounds that finished: {said}"
        );
        assert_eq!(live.iteration(), 2);
    }

    /// A delegate's run isn't on disk, and a background one outlives the
    /// round that started it, so a late view gets all of it.
    #[tokio::test]
    async fn a_view_arriving_late_gets_every_delegate_of_the_turn() {
        use taurus_tools::DelegateStep;
        let live = Live::new();
        live.publish(UiEvent::IterationStarted { iteration: 1 })
            .await;
        live.publish(UiEvent::ToolDetached { id: "bg".into() })
            .await;
        live.publish(UiEvent::DelegateActivity {
            id: "bg".into(),
            step: DelegateStep::Text {
                text: "Reading ".into(),
            },
        })
        .await;
        live.publish(UiEvent::DelegateActivity {
            id: "bg".into(),
            step: DelegateStep::Text {
                text: "the config.".into(),
            },
        })
        .await;
        live.publish(UiEvent::IterationStarted { iteration: 2 })
            .await;
        live.publish(text("the parent goes on")).await;

        let late = View::default();
        live.attach(late.open()).await;
        let said = late.said();
        assert!(said.contains("tool_detached"), "{said}");
        // Merged, as the parent's own deltas are.
        assert!(said.contains("Reading the config."), "{said}");
        // The round comes first, so the card exists before its delegate does.
        assert!(
            said.find("the parent goes on") < said.find("tool_detached"),
            "{said}"
        );
    }

    /// A reload replaces every view. The old page hears nothing more, and the
    /// new one picks the round up from the replay.
    #[tokio::test]
    async fn a_page_that_was_replaced_is_let_go_and_the_next_one_catches_up() {
        let live = Live::new();
        let old = View::default();
        live.attach(old.open()).await;
        live.publish(text("before the reload")).await;

        live.forget_views().await;
        live.publish(text(" and after")).await;
        assert_eq!(
            old.seen(),
            1,
            "a page that has gone was still being sent to"
        );

        let new = View::default();
        assert_eq!(live.attach(new.open()).await, 0);
        assert!(new.said().contains("before the reload and after"));
    }

    /// A conversation nobody has opened yet still runs, and the replay is what
    /// it keeps for whoever turns up.
    #[tokio::test]
    async fn a_turn_nobody_is_watching_keeps_going() {
        let live = Live::new();
        live.publish(text("talking to nobody")).await;

        let late = View::default();
        live.attach(late.open()).await;

        assert!(late.said().contains("talking to nobody"));
    }

    fn text(body: &str) -> UiEvent {
        UiEvent::TextDelta {
            text: body.to_string(),
        }
    }

    fn held(replay: &Replay) -> Vec<String> {
        replay
            .events
            .iter()
            .map(|event| match event {
                UiEvent::TextDelta { text } | UiEvent::ThinkingDelta { text } => text.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn consecutive_deltas_of_one_kind_are_one_delta() {
        let mut replay = Replay::default();
        replay.push(text("Look"));
        replay.push(text("ing at "));
        replay.push(text("the file"));

        assert_eq!(held(&replay), vec!["Looking at the file"]);
    }

    /// The merge is per kind and per run: reasoning that resumes after an
    /// answer is a second block, not an addition to the first.
    #[test]
    fn a_different_kind_between_them_breaks_the_run() {
        let mut replay = Replay::default();
        replay.push(text("one"));
        replay.push(UiEvent::ThinkingDelta {
            text: "hmm".to_string(),
        });
        replay.push(text("two"));

        assert_eq!(held(&replay), vec!["one", "hmm", "two"]);
    }

    /// What a round holds is what the transcript is missing, so a round
    /// beginning is the moment the previous one stops being worth keeping.
    #[test]
    fn a_new_round_forgets_the_one_before_it() {
        let mut replay = Replay::default();
        replay.push(text("first round"));
        replay.clear();
        replay.push(text("second round"));

        assert_eq!(held(&replay), vec!["second round"]);
        assert_eq!(replay.dropped, 0);
    }

    #[test]
    fn the_oldest_goes_once_the_cap_is_reached() {
        let mut replay = Replay::default();
        let big = "x".repeat(MAX_REPLAY_BYTES / 2);
        // Separated by another kind, or they would merge into one event that
        // dropping cannot help with.
        for _ in 0..3 {
            replay.push(text(&big));
            replay.push(UiEvent::ToolProgress {
                id: "call".to_string(),
                label: "working".to_string(),
            });
        }

        assert!(replay.bytes <= MAX_REPLAY_BYTES);
        assert!(replay.dropped > 0, "a view arriving late has to be told");
    }

    /// Merging is what keeps the cap measuring text rather than event count: a
    /// model streaming a page of prose in 16-millisecond pieces would otherwise
    /// spend the whole buffer on overhead.
    #[test]
    fn a_page_of_prose_in_small_pieces_costs_what_the_prose_costs() {
        let mut replay = Replay::default();
        for _ in 0..4000 {
            replay.push(text("word "));
        }

        assert_eq!(replay.dropped, 0);
        assert_eq!(replay.events.len(), 1);
        assert!(replay.bytes < 4000 * EVENT_OVERHEAD);
    }
}
