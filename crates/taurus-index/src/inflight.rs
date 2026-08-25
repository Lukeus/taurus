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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

/// The one refresh that may be in flight for a workspace.
#[derive(Default)]
pub struct Indexing {
    /// The running refresh and the number it was given, so a refresh that
    /// finishes after being taken over does not clear its successor.
    current: Mutex<Option<(u64, CancellationToken)>>,
    next: AtomicU64,
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
    /// the refresh returns, and hand it back then.
    pub fn take_over(&self, cancel: &CancellationToken) -> u64 {
        let ticket = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some((_, previous)) = self
            .current
            .lock()
            .unwrap()
            .replace((ticket, cancel.clone()))
        {
            previous.cancel();
        }
        ticket
    }

    /// Records that a refresh is over, if it is still the one running.
    pub fn finished(&self, ticket: u64) {
        let mut current = self.current.lock().unwrap();
        if current.as_ref().is_some_and(|(held, _)| *held == ticket) {
            *current = None;
        }
    }

    /// Stops whatever is refreshing, for a workspace being left.
    pub fn stop(&self) {
        if let Some((_, cancel)) = self.current.lock().unwrap().take() {
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
        flight.take_over(&first);
        assert!(flight.busy());

        let second = CancellationToken::new();
        flight.take_over(&second);
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
    }

    #[test]
    fn a_refresh_that_was_taken_over_does_not_clear_its_successor() {
        // The warm-up finishing its last batch after a search took over would
        // otherwise leave the search unregistered, and a third refresh would
        // start beside it.
        let flight = Indexing::new();
        let first = flight.take_over(&CancellationToken::new());
        let second = flight.take_over(&CancellationToken::new());

        flight.finished(first);
        assert!(flight.busy(), "the running refresh was forgotten");

        flight.finished(second);
        assert!(!flight.busy());
    }

    #[test]
    fn stopping_leaves_nothing_running() {
        let flight = Indexing::new();
        let cancel = CancellationToken::new();
        flight.take_over(&cancel);
        flight.stop();
        assert!(cancel.is_cancelled());
        assert!(!flight.busy());
    }
}
