//! What the watcher has seen, and the one rule for answering from it.
//!
//! Every path an event names is written down with a sequence number. A
//! [`Mark`] is a sequence number handed to a consumer, and asking what changed
//! since it is a lookup — every path written after it.
//!
//! The rule is the one git's fsmonitor protocol is built on: an answer is
//! either a list of paths that is complete, or [`Since::Everything`]. There is
//! no third answer meaning "these, probably". When the operating system says
//! it dropped events, when the watcher errors, or when the journal would grow
//! past what it keeps, every mark taken before that moment is answered with
//! `Everything`, and the consumer goes and looks for itself. That is what makes
//! the fast answer safe to act on: it is only ever given when it is whole.
//!
//! None of this touches the filesystem, so all of it is tested here without
//! a watcher running.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Most distinct paths held between two questions.
///
/// A build writes thousands of files under `target/` or `node_modules/`, and
/// the watcher writes all of them down: whether a directory is one the sweep
/// covers is the consumer's question, not this one's. Past this the journal
/// starts over and answers `Everything` once, which costs one full walk —
/// cheaper than holding an unbounded map of a build's output for as long as
/// the workspace is open.
pub(crate) const MAX_PATHS: usize = 100_000;

/// A point in the journal, to ask what changed after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    seq: u64,
}

/// What changed since a [`Mark`].
#[derive(Debug, PartialEq, Eq)]
pub enum Since {
    /// Every path an event named after the mark, sorted. Complete: a change
    /// that is not here did not happen, as far as the operating system said.
    /// A path may be here without having changed — an event names what was
    /// touched — so a consumer still compares what it finds.
    Paths(Vec<PathBuf>),
    /// The journal cannot say, and here is why. The consumer looks for itself.
    Everything(String),
}

#[derive(Default)]
pub(crate) struct Journal {
    seq: u64,
    /// Marks below this were taken before something was lost.
    floor: u64,
    /// Why the floor last moved, for the `Everything` it causes.
    lost: String,
    /// Set once and never cleared: the watcher can no longer see changes at
    /// all, such as when the operating system's watch limit was reached.
    broken: Option<String>,
    /// Each path's latest sequence number.
    paths: HashMap<PathBuf, u64>,
    /// Cookies a caller is waiting on, and those that have arrived.
    pending: HashSet<String>,
    arrived: HashSet<String>,
}

impl Journal {
    pub(crate) fn mark(&self) -> Mark {
        Mark { seq: self.seq }
    }

    pub(crate) fn record(&mut self, path: PathBuf) {
        self.seq += 1;
        self.paths.insert(path, self.seq);
        if self.paths.len() > MAX_PATHS {
            self.lose(format!(
                "more than {MAX_PATHS} paths changed between two looks, more than the watcher \
                 keeps track of"
            ));
        }
    }

    /// Everything before now is uncertain. Every mark already handed out is
    /// answered with `Everything`; marks taken from here on are good again.
    pub(crate) fn lose(&mut self, reason: String) {
        self.seq += 1;
        self.floor = self.seq;
        self.lost = reason;
        self.paths.clear();
    }

    /// The watcher can no longer see changes. Unlike [`Journal::lose`], new
    /// marks do not recover from this.
    pub(crate) fn break_with(&mut self, reason: String) {
        if self.broken.is_none() {
            self.broken = Some(reason);
        }
        self.paths.clear();
    }

    pub(crate) fn broken(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    pub(crate) fn since(&self, mark: Mark) -> Since {
        if let Some(reason) = &self.broken {
            return Since::Everything(reason.clone());
        }
        if mark.seq < self.floor {
            return Since::Everything(self.lost.clone());
        }
        let mut paths: Vec<PathBuf> = self
            .paths
            .iter()
            .filter(|(_, &seq)| seq > mark.seq)
            .map(|(path, _)| path.clone())
            .collect();
        paths.sort();
        Since::Paths(paths)
    }

    pub(crate) fn expect_cookie(&mut self, name: String) {
        self.pending.insert(name);
    }

    /// A cookie's event arrived. Only the ones someone is waiting on are kept:
    /// the same cookie's removal arrives a moment later, and a waiter that
    /// timed out has already stopped waiting.
    pub(crate) fn cookie_seen(&mut self, name: &str) {
        if self.pending.remove(name) {
            self.arrived.insert(name.to_string());
        }
    }

    /// Whether the cookie arrived, forgetting it either way once it has.
    pub(crate) fn take_cookie(&mut self, name: &str) -> bool {
        self.arrived.remove(name)
    }

    /// Stops waiting on a cookie that never arrived.
    pub(crate) fn abandon_cookie(&mut self, name: &str) {
        self.pending.remove(name);
        self.arrived.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(since: Since) -> Vec<String> {
        match since {
            Since::Paths(paths) => paths.iter().map(|p| p.display().to_string()).collect(),
            Since::Everything(reason) => panic!("expected paths, got everything: {reason}"),
        }
    }

    #[test]
    fn a_mark_sees_only_what_came_after_it() {
        let mut journal = Journal::default();
        journal.record("a".into());
        let mark = journal.mark();
        journal.record("b".into());
        journal.record("c".into());
        assert_eq!(paths(journal.since(mark)), ["b", "c"]);
    }

    #[test]
    fn a_path_touched_again_after_the_mark_is_reported_once() {
        let mut journal = Journal::default();
        journal.record("a".into());
        let mark = journal.mark();
        journal.record("a".into());
        journal.record("a".into());
        assert_eq!(paths(journal.since(mark)), ["a"]);
    }

    #[test]
    fn answers_are_sorted() {
        let mut journal = Journal::default();
        let mark = journal.mark();
        for name in ["c", "a", "b"] {
            journal.record(name.into());
        }
        assert_eq!(paths(journal.since(mark)), ["a", "b", "c"]);
    }

    #[test]
    fn a_loss_answers_every_earlier_mark_with_everything() {
        let mut journal = Journal::default();
        let early = journal.mark();
        journal.record("a".into());
        journal.lose("the kernel dropped events".into());
        assert_eq!(
            journal.since(early),
            Since::Everything("the kernel dropped events".into())
        );
    }

    #[test]
    fn a_mark_taken_after_a_loss_is_good_again() {
        let mut journal = Journal::default();
        journal.record("a".into());
        journal.lose("dropped".into());
        let after = journal.mark();
        journal.record("b".into());
        assert_eq!(paths(journal.since(after)), ["b"]);
    }

    #[test]
    fn breaking_answers_every_mark_with_everything_for_good() {
        let mut journal = Journal::default();
        journal.break_with("the watch limit was reached".into());
        let after = journal.mark();
        journal.record("a".into());
        assert_eq!(
            journal.since(after),
            Since::Everything("the watch limit was reached".into())
        );
    }

    #[test]
    fn the_first_reason_a_watcher_broke_is_the_one_kept() {
        let mut journal = Journal::default();
        journal.break_with("first".into());
        journal.break_with("second".into());
        assert_eq!(journal.broken(), Some("first"));
    }

    #[test]
    fn growing_past_the_cap_starts_over_rather_than_dropping_paths() {
        let mut journal = Journal::default();
        let mark = journal.mark();
        for n in 0..=MAX_PATHS {
            journal.record(format!("f{n}").into());
        }
        assert!(matches!(journal.since(mark), Since::Everything(_)));
        assert!(journal.paths.is_empty());
    }

    #[test]
    fn a_cookie_counts_only_when_someone_waits_on_it() {
        let mut journal = Journal::default();
        journal.cookie_seen("stray");
        assert!(!journal.take_cookie("stray"));

        journal.expect_cookie("one".into());
        journal.cookie_seen("one");
        assert!(journal.take_cookie("one"));
        // Its removal event arrives afterwards and is not a second arrival.
        journal.cookie_seen("one");
        assert!(!journal.take_cookie("one"));
    }

    #[test]
    fn an_abandoned_cookie_arriving_late_is_not_held() {
        let mut journal = Journal::default();
        journal.expect_cookie("late".into());
        journal.abandon_cookie("late");
        journal.cookie_seen("late");
        assert!(!journal.take_cookie("late"));
    }
}
