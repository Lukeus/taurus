//! What the spans said, kept where a window can read it.
//!
//! The spans next door are written for a collector: they are opened, recorded
//! onto, and closed, and if nobody subscribed they cost a few atomic
//! operations and vanish. That is the right default for an exporter — see the
//! module docs — but it leaves the app with nothing to draw. A dashboard of
//! *this machine's* last few turns cannot be built out of spans that were
//! never kept, and asking somebody to stand up a collector before they can
//! find out why a turn took ninety seconds is asking for the wrong thing at
//! the wrong moment.
//!
//! So this is a second destination, and a very small one: a bounded ring of
//! finished spans in memory, in this process, which nothing sends anywhere. It
//! is not a substitute for a collector — it forgets on quit and holds
//! [`CAPACITY`] records — and it is not meant to be. It is the local read.
//!
//! # Why records rather than the spans themselves
//!
//! A `tracing` span id is the subscriber's, and it is *reused* once the span
//! closes. Building a tree out of those ids would be correct for as long as a
//! process is short and wrong the moment it is not: a parent id stored beside
//! a child would eventually name a different, later span, and the waterfall
//! would draw one turn's tool call underneath another turn. So each record
//! gets a sequence number of its own here, handed out once and never reused,
//! and the parent link is in those terms.
//!
//! # Nothing leaves the machine
//!
//! Worth saying plainly, because "telemetry that is on by default" is a phrase
//! with a history. This is a ring buffer in the process that produced it. It
//! has no endpoint, no file, and no lifetime past the window being closed, and
//! it holds what the spans hold — a model name, a tool name, durations and
//! token counts — never the conversation. [`Capture`](super::Capture) governs
//! the messages and it governs them here too, by simply not being read: the
//! two message fields are not among the ones a record has room for.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How many finished spans are kept before the oldest is forgotten.
///
/// A turn of ordinary size is a dozen or two records, so this is a few hundred
/// turns — long enough that the answer to "why was that slow" is still here
/// after a morning's work, small enough that nobody has to think about what
/// leaving the app open costs. What falls off the end is counted rather than
/// silently dropped, because a dashboard that has quietly stopped covering the
/// period it appears to cover is worse than one that says so.
pub const CAPACITY: usize = 4096;

/// Which of the three spans a record came from.
///
/// The names are the operations, not the span names, because that is what the
/// distinction is for: a model call, a tool call, and the turn around them
/// cost time for entirely different reasons and are read differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SpanKind {
    /// `invoke_agent` — one whole turn, parent to everything below it.
    Turn,
    /// `chat` — one request to the model. One *attempt*: a retried request is
    /// two of these, because it was two round trips.
    Chat,
    /// `execute_tool` — one tool call. A `spawn` is one of these with a
    /// delegate's whole turn nested inside it.
    Tool,
}

/// One finished span, as much of it as is worth keeping.
///
/// Everything optional is optional because the harness genuinely may not know
/// it: a provider that reports no cache has no cache figure, and a turn that
/// was cancelled has no finish reason worth the name. Absent and zero are
/// different facts here for the same reason they are in
/// [`record_usage`](super::record_usage) — a zero would put a cache hit rate
/// on a dashboard for a backend that has no cache.
#[derive(Clone, Debug)]
pub struct SpanRecord {
    /// This process's own numbering, handed out once. See the module docs for
    /// why the subscriber's span id will not do.
    pub seq: u64,
    /// The nearest ancestor that was also recorded, if there was one.
    pub parent: Option<u64>,
    pub kind: SpanKind,
    /// The tool's name, or the model's. What a reader scans the column for.
    pub name: String,
    pub provider: Option<String>,
    /// The conversation this belongs to, for spans that name one. A tool call
    /// does not; it inherits one from the turn above it when the report is
    /// built.
    pub conversation: Option<String>,
    /// When it opened, in milliseconds since the epoch.
    ///
    /// Wall clock rather than a monotonic instant, because it has to be
    /// comparable with the conversation on screen and printable as a time of
    /// day. The *duration* beside it is measured monotonically, so a clock
    /// that steps mid-turn moves the mark on the timeline and never produces a
    /// span that took negative time.
    pub started: u64,
    pub duration_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    /// The conventions' word for why it stopped — `stop`, `tool_calls`,
    /// `length`, or this harness's own `canceled`.
    pub finish: Option<String>,
    /// The kind of failure that ended it, if one did. Low-cardinality by
    /// construction: it is `error.type`, not the message.
    pub error: Option<String>,
}

/// The ring, shareable and cheap to clone.
///
/// A `std::sync::Mutex` rather than an async one on purpose. Every hold is a
/// push of one record or a snapshot of a bounded buffer — microseconds, no
/// await inside — and making a span's close point depend on a runtime being
/// alive would be the wrong dependency in the one place that has to work while
/// the process is shutting down.
#[derive(Clone, Default)]
pub struct Traces(Arc<Mutex<Ring>>);

#[derive(Default)]
struct Ring {
    records: VecDeque<SpanRecord>,
    dropped: u64,
}

/// Everything retained, and what was not.
pub struct Snapshot {
    /// Oldest first, which is the order they finished in.
    pub records: Vec<SpanRecord>,
    /// How many fell off the end since the process started.
    pub dropped: u64,
}

impl Traces {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps one finished span, forgetting the oldest if the ring is full.
    pub fn push(&self, record: SpanRecord) {
        let mut ring = self.lock();
        if ring.records.len() >= CAPACITY {
            ring.records.pop_front();
            ring.dropped += 1;
        }
        ring.records.push_back(record);
    }

    /// A copy of what is held, for a reader that is about to do arithmetic on
    /// it.
    ///
    /// Copied rather than read under the lock, because the alternative is a
    /// report built while a turn is running holding up the span that turn is
    /// trying to close. The buffer is bounded, so the copy is too.
    pub fn snapshot(&self) -> Snapshot {
        let ring = self.lock();
        Snapshot {
            records: ring.records.iter().cloned().collect(),
            dropped: ring.dropped,
        }
    }

    /// Forgets everything, including the count of what was forgotten.
    ///
    /// For a person who wants the next measurement to be of the next thing
    /// they do, rather than of it plus this morning.
    pub fn clear(&self) {
        let mut ring = self.lock();
        ring.records.clear();
        ring.dropped = 0;
    }

    /// The lock, with a poisoned one taken anyway.
    ///
    /// A panic elsewhere must not turn this into a second panic on every span
    /// that closes afterwards. The worst a poisoned ring can hold is a
    /// half-updated `dropped` count, and refusing to record anything ever
    /// again would be a far larger failure than an off-by-one in a figure that
    /// exists to say "there was more than this".
    fn lock(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(seq: u64) -> SpanRecord {
        SpanRecord {
            seq,
            parent: None,
            kind: SpanKind::Tool,
            name: "read_file".into(),
            provider: None,
            conversation: None,
            started: 1_000 + seq,
            duration_ms: 5,
            input_tokens: None,
            output_tokens: None,
            cached_tokens: None,
            finish: None,
            error: None,
        }
    }

    #[test]
    fn a_full_ring_forgets_the_oldest_and_says_how_many() {
        // The counting is the point. A dashboard that has quietly stopped
        // covering the period it appears to cover is worse than one that says
        // so, and "since the app started" is exactly the claim a full ring can
        // no longer make.
        let traces = Traces::new();
        for seq in 0..(CAPACITY as u64 + 3) {
            traces.push(record(seq));
        }

        let snapshot = traces.snapshot();
        assert_eq!(snapshot.records.len(), CAPACITY);
        assert_eq!(snapshot.dropped, 3);
        assert_eq!(snapshot.records[0].seq, 3, "the oldest three went");
    }

    #[test]
    fn a_snapshot_is_oldest_first() {
        // The order they finished in, which is what a timeline is drawn from.
        let traces = Traces::new();
        for seq in 0..4 {
            traces.push(record(seq));
        }
        let seqs: Vec<u64> = traces.snapshot().records.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3]);
    }

    #[test]
    fn clearing_forgets_that_anything_was_forgotten() {
        // Otherwise the panel would go on reporting "and 300 more" about a
        // window that now starts from nothing.
        let traces = Traces::new();
        for seq in 0..(CAPACITY as u64 + 1) {
            traces.push(record(seq));
        }
        traces.clear();

        let snapshot = traces.snapshot();
        assert!(snapshot.records.is_empty());
        assert_eq!(snapshot.dropped, 0);
    }

    #[test]
    fn a_handle_shares_one_ring_rather_than_copying_it() {
        // The layer holds one of these and the command that draws the panel
        // holds another. Two rings would mean a dashboard of nothing at all.
        let traces = Traces::new();
        let other = traces.clone();
        other.push(record(1));
        assert_eq!(traces.snapshot().records.len(), 1);
    }
}
