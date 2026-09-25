//! Work a turn started and hasn't heard back from yet.
//!
//! A background delegate returns from its tool call at once and reports later,
//! into the turn that started it. That turn has to know three things: what has
//! arrived, whether anything is still out, and how to stop what is. This holds
//! all three, one per turn.
//!
//! Scoped to the turn on purpose. Work that outlived its turn would need
//! somewhere to report that isn't a turn, and the conversation would need a
//! way to start one from an event instead of a message. Neither exists, and a
//! turn that waits for what it started needs neither.

use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// A report that came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// The tool call that started the work.
    pub call: String,
    /// What did it, as the reader should see it named: `explorer`.
    pub agent: String,
    /// The report, as the model reads it.
    pub text: String,
}

impl Arrival {
    /// The block the model reads. Tagged so a reopened conversation can tell
    /// a report the harness delivered from a message the user typed, and put
    /// it back on the card it belongs to. See `src/lib/delegate.ts`.
    pub fn render(&self) -> String {
        format!(
            "<background-report call=\"{}\" agent=\"{}\">\n{}\n</background-report>",
            self.call,
            self.agent,
            self.text.trim()
        )
    }
}

#[derive(Default)]
struct State {
    running: usize,
    arrived: Vec<Arrival>,
}

/// One turn's outstanding work. See the module docs.
pub struct Pending {
    state: Mutex<State>,
    changed: Notify,
    /// A child of the turn's own, so Stop reaches the work, and so the turn
    /// can stop it on any other way out without cancelling itself.
    stop: CancellationToken,
}

/// Held by one piece of outstanding work until it reports.
///
/// Dropped without a report — a panic, an early return — it still counts the
/// work as done, so nothing waits on it forever.
///
/// Taken before the tool call that starts the work returns, so the turn never
/// sees a moment where the work is out but not counted, and moved into the
/// task that does it.
pub struct Ticket {
    pending: Arc<Pending>,
    done: bool,
}

impl Pending {
    pub fn new(turn: &CancellationToken) -> Self {
        Self {
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            stop: turn.child_token(),
        }
    }

    /// The token outstanding work runs under.
    pub fn stop_token(&self) -> CancellationToken {
        self.stop.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Counts one more piece of work as out.
    pub fn start(self: &Arc<Self>) -> Ticket {
        self.state().running += 1;
        Ticket {
            pending: self.clone(),
            done: false,
        }
    }

    /// How much is still out.
    pub fn running(&self) -> usize {
        self.state().running
    }

    /// Whether anything has arrived that hasn't been taken.
    pub fn has_arrived(&self) -> bool {
        !self.state().arrived.is_empty()
    }

    /// Everything that has arrived since the last call, oldest first.
    pub fn take(&self) -> Vec<Arrival> {
        std::mem::take(&mut self.state().arrived)
    }

    /// Waits until something arrives or nothing is left out.
    ///
    /// Returns at once if either is already true, so a caller can't miss a
    /// report that landed before it asked.
    pub async fn wait(&self) {
        loop {
            let notified = self.changed.notified();
            {
                let state = self.state();
                if !state.arrived.is_empty() || state.running == 0 {
                    return;
                }
            }
            notified.await;
        }
    }

    /// Stops everything still out and waits for it to wind down.
    ///
    /// For every way a turn ends other than the one that waits for its work:
    /// an error, the iteration ceiling, a stall. The work shares the turn's
    /// event stream, and a turn whose work is still writing to it hasn't
    /// really ended. What arrives in the meantime is dropped with the turn.
    pub async fn stop_all(&self) {
        self.stop.cancel();
        loop {
            let notified = self.changed.notified();
            if self.running() == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl Ticket {
    /// Hands the report back and counts the work as done.
    pub fn deliver(mut self, arrival: Arrival) {
        self.done = true;
        let mut state = self.pending.state();
        state.running -= 1;
        state.arrived.push(arrival);
        drop(state);
        self.pending.changed.notify_waiters();
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.done {
            self.pending.state().running -= 1;
            self.pending.changed.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn arrival(call: &str) -> Arrival {
        Arrival {
            call: call.into(),
            agent: "explorer".into(),
            text: "Status: done.\n\nFound it.".into(),
        }
    }

    #[tokio::test]
    async fn waiting_returns_when_a_report_arrives() {
        let pending = Arc::new(Pending::new(&CancellationToken::new()));
        let ticket = pending.start();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            ticket.deliver(arrival("c1"));
        });
        assert_eq!(pending.running(), 1);
        tokio::time::timeout(Duration::from_secs(2), pending.wait())
            .await
            .expect("a report must end the wait");
        task.await.unwrap();
        assert_eq!(pending.take(), vec![arrival("c1")]);
        assert_eq!(pending.running(), 0);
        assert!(pending.take().is_empty(), "each report is taken once");
    }

    #[tokio::test]
    async fn waiting_with_nothing_out_returns_at_once() {
        let pending = Pending::new(&CancellationToken::new());
        tokio::time::timeout(Duration::from_millis(100), pending.wait())
            .await
            .expect("nothing out, nothing to wait for");
    }

    #[tokio::test]
    async fn a_ticket_dropped_without_a_report_still_counts_as_done() {
        let pending = Arc::new(Pending::new(&CancellationToken::new()));
        drop(pending.start());
        assert_eq!(pending.running(), 0);
    }

    #[tokio::test]
    async fn stop_reaches_the_work_and_waits_for_it() {
        let turn = CancellationToken::new();
        let pending = Arc::new(Pending::new(&turn));
        let ticket = pending.start();
        let token = pending.stop_token();
        tokio::spawn(async move {
            token.cancelled().await;
            drop(ticket);
        });
        tokio::time::timeout(Duration::from_secs(2), pending.stop_all())
            .await
            .expect("stopped work must wind down");
        assert!(
            !turn.is_cancelled(),
            "stopping the work is not stopping the turn"
        );
    }

    #[test]
    fn a_report_is_tagged_with_its_call() {
        assert_eq!(
            arrival("c1").render(),
            "<background-report call=\"c1\" agent=\"explorer\">\nStatus: done.\n\nFound it.\n</background-report>"
        );
    }
}
