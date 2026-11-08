//! Which refresh of a workspace's index is the one running.
//!
//! There are three things that refresh an index — the **Build index** button,
//! a `search_code` call, and the warm-up a turn starts — and they all write the
//! same file. Two at once is not a correctness problem, because the last write
//! is whole either way, but it is the same thousands of embedding requests sent
//! twice while a turn waits for one of them.
//!
//! So there is one, and starting another stops it. The order is deliberate:
//! whoever is asking now has somebody waiting on the answer, and the refresh it
//! interrupts keeps everything it had embedded ([`crate::build`] writes as it
//! goes), so taking over costs the seconds since the last write rather than the
//! run.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

/// The one refresh that may be in flight for a workspace.
#[derive(Default)]
pub struct Indexing {
    /// The running refresh, the number it was given (so a refresh that
    /// finishes after being taken over does not clear its successor), and
    /// what it shares with anyone who wants to search before it's done.
    current: Mutex<Option<(u64, CancellationToken, Arc<Live>)>>,
    next: AtomicU64,
}

/// What a running refresh shares with a search that won't wait for it.
///
/// Handed to [`crate::refresh`] as its progress. A refresh started without
/// it, like **Build index** with its own progress bar, still runs; a search
/// joining it just can't ask it to write down early, and answers from its
/// last periodic write instead.
#[derive(Default)]
pub struct Live {
    /// `(done, total)` passages.
    pub(crate) progress: tokio::sync::watch::Sender<(usize, usize)>,
    /// What the scan found, once it knows: `(kept, total)` passages already
    /// current and to embed. `(0, 0)` until then.
    pub(crate) planned: tokio::sync::watch::Sender<(usize, usize)>,
    pub(crate) flush: AtomicBool,
    pub(crate) flushed: tokio::sync::Notify,
}

impl Live {
    /// Passages the scan found to embed, `None` before it has finished.
    pub fn planned(&self) -> Option<usize> {
        let (_, total) = *self.planned.borrow();
        (total > 0).then_some(total)
    }

    /// Asks for what's finished to be written down at the next batch.
    pub(crate) fn request_flush(&self) {
        self.flush.store(true, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl crate::build::IndexProgress for Live {
    async fn embedding(&self, done: usize, total: usize) {
        self.progress.send_replace((done, total));
    }

    fn planned(&self, kept: usize, total: usize) {
        self.planned.send_replace((kept, total));
        self.progress.send_replace((0, total));
    }

    fn flush_wanted(&self) -> bool {
        self.flush.load(Ordering::SeqCst)
    }

    fn flushed(&self) {
        self.flush.store(false, Ordering::SeqCst);
        // A permit if nobody is waiting yet, so a search can't miss it.
        self.flushed.notify_one();
    }
}

impl Indexing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a refresh is running, for the caller deciding to start one.
    pub fn busy(&self) -> bool {
        self.current.lock().unwrap().is_some()
    }

    /// Stops whatever was refreshing and puts this one in its place.
    ///
    /// The returned number is what [`Indexing::finished`] takes: hold it until
    /// the refresh returns, and hand it back then. The returned token is the
    /// one to refresh with: a child of `cancel`, so the caller's Stop still
    /// reaches the refresh, and a later takeover stops only the refresh. Held
    /// as `cancel` itself, a takeover would cancel whatever else `cancel`
    /// governs, which for `search_code` is the whole turn.
    pub fn take_over(&self, cancel: &CancellationToken) -> (u64, CancellationToken) {
        let (ticket, own, _) = self.take_over_live(cancel);
        (ticket, own)
    }

    /// [`Indexing::take_over`], plus the [`Live`] to refresh with, so a
    /// search can join this refresh rather than restart it.
    pub fn take_over_live(
        &self,
        cancel: &CancellationToken,
    ) -> (u64, CancellationToken, Arc<Live>) {
        let ticket = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let own = cancel.child_token();
        let live = Arc::new(Live::default());
        if let Some((_, previous, _)) =
            self.current
                .lock()
                .unwrap()
                .replace((ticket, own.clone(), Arc::clone(&live)))
        {
            previous.cancel();
        }
        (ticket, own, live)
    }

    /// The running refresh's [`Live`], if one is running.
    pub fn running(&self) -> Option<Arc<Live>> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, _, live)| Arc::clone(live))
    }

    /// Records that a refresh is over, if it is still the one running.
    pub fn finished(&self, ticket: u64) {
        let mut current = self.current.lock().unwrap();
        if current.as_ref().is_some_and(|(held, _, _)| *held == ticket) {
            *current = None;
        }
    }

    /// Stops whatever is refreshing, for a workspace being left.
    pub fn stop(&self) {
        if let Some((_, cancel, _)) = self.current.lock().unwrap().take() {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_refresh_stops_the_first() {
        let flight = Indexing::new();
        let first = CancellationToken::new();
        let (_, first_own) = flight.take_over(&first);
        assert!(flight.busy());

        let second = CancellationToken::new();
        let (_, second_own) = flight.take_over(&second);
        assert!(first_own.is_cancelled());
        assert!(!second_own.is_cancelled());
        // Only the refresh stops. The token it was started under governs more
        // than the refresh: a turn, and every other tool call in it.
        assert!(!first.is_cancelled());
    }

    #[test]
    fn the_callers_stop_still_reaches_the_refresh() {
        let flight = Indexing::new();
        let turn = CancellationToken::new();
        let (_, own) = flight.take_over(&turn);
        turn.cancel();
        assert!(own.is_cancelled());
    }

    #[test]
    fn a_refresh_that_was_taken_over_does_not_clear_its_successor() {
        // The warm-up finishing its last batch after a search took over would
        // otherwise leave the search unregistered, and a third refresh would
        // start beside it.
        let flight = Indexing::new();
        let (first, _) = flight.take_over(&CancellationToken::new());
        let (second, _) = flight.take_over(&CancellationToken::new());

        flight.finished(first);
        assert!(flight.busy(), "the running refresh was forgotten");

        flight.finished(second);
        assert!(!flight.busy());
    }

    #[test]
    fn stopping_leaves_nothing_running() {
        let flight = Indexing::new();
        let cancel = CancellationToken::new();
        let (_, own) = flight.take_over(&cancel);
        flight.stop();
        assert!(own.is_cancelled());
        assert!(!cancel.is_cancelled());
        assert!(!flight.busy());
    }
}
