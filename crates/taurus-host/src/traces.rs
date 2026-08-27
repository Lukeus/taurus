//! Where a turn's time actually went.
//!
//! [`crate::usage`] answers "what filled the context window", which is a
//! question about tokens and is read out of the transcript. This answers the
//! other one — *why did that take ninety seconds* — and it cannot be read out
//! of the transcript at all, because a transcript records what was said and not
//! how long any of it took. The source is the span ring in
//! [`taurus_core::telemetry::store`], filled by whatever the process just did.
//!
//! It lives here rather than in the app because this is where a report is
//! assembled — the arithmetic, the ordering and the shares are settled once,
//! and what crosses the boundary is finished. Unlike the usage account it has
//! only one caller today, and that is a property of the source rather than an
//! oversight: the ring belongs to the process that filled it, so a `taurus
//! traces` typed at a shell would describe a program that has not done
//! anything yet. The terminal's route to the same spans is an OTLP endpoint.
//!
//! # The split at the top, and why it is not "model versus tools"
//!
//! A turn's wall time divides cleanly in exactly one place: time inside a
//! `chat` span, and everything else. Model calls never nest inside one
//! another, so summing them is safe.
//!
//! Tool time is not safe to sum, and the reason is delegation. A `spawn` is one
//! tool call with an entire sub-agent's turn inside it — its model calls, its
//! own tools, all of it. Adding `spawn`'s duration to the durations of the
//! tools that ran inside it counts the same seconds twice, and a panel whose
//! parts add up to 180% of a turn is a panel nobody can reason from. So the
//! headline is model time against **everything else**, where everything else is
//! honestly named: tools doing their own work, the harness between steps, and
//! whatever the turn spent waiting.
//!
//! The per-tool table below it is a separate view with its own denominator,
//! and it does include `spawn` — because "delegation took forty seconds" is
//! worth seeing. Within that table nothing is counted twice; across the two,
//! the numbers are answering different questions and are not meant to add up.
//!
//! # Why every number crosses as a `number`
//!
//! Durations, token counts and timestamps are `u64` here and `number` in
//! TypeScript, declared field by field. Left alone, ts-rs maps a `u64` to
//! `bigint`, which is the correct type and the wrong one to hand a frontend:
//! every arithmetic in the panel would need a conversion, `toLocaleString`
//! would behave differently, and `JSON.stringify` throws on one. A double
//! holds a millisecond count exactly past any duration a turn will ever have
//! and a millisecond timestamp exactly past the year 285-million-odd, so
//! nothing is lost for precision anybody will use.
//!
//! # Median and slowest, not p95
//!
//! A percentile over eleven samples is a number with a decimal point and no
//! information in it. What a local ring holds is tens or hundreds of calls, not
//! the millions a percentile is meaningful over, so what is reported is the
//! middle one and the worst one — both of which are real calls that really
//! happened, and the second of which is usually the one being looked for.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use taurus_core::telemetry::{SpanKind, SpanRecord, Traces};

/// How many recent turns are described step by step.
///
/// The waterfall is the expensive half of this payload — a dozen turns of a
/// dozen steps each — and the far end of it is not what anybody is looking at.
/// The aggregates above it are computed over everything retained regardless.
const TURNS_DESCRIBED: usize = 12;

/// One model call's worth of a model's record.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelLatency {
    /// The model that answered, which is not always the one that was asked
    /// for: a backend serving an alias reports something else, and that is
    /// worth seeing.
    pub name: String,
    pub provider: String,
    /// Requests, not turns. A retried request is two.
    pub calls: u32,
    #[ts(type = "number")]
    pub median_ms: u64,
    #[ts(type = "number")]
    pub slowest_ms: u64,
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    /// Of `input_tokens`, what came from the provider's prompt cache. `None`
    /// until some backend reports one — a local Ollama has no cache to have
    /// missed, and a 0% hit rate beside its name invites the wrong conclusion.
    #[ts(type = "number | null")]
    pub cached_tokens: Option<u64>,
    pub failures: u32,
    /// Output tokens per second across every call to it, or `None` when
    /// nothing was measurable. The number people compare backends with.
    pub output_per_second: Option<u32>,
}

/// What one tool cost in time, across every call to it that is still retained.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolLatency {
    pub name: String,
    pub calls: u32,
    pub failures: u32,
    #[ts(type = "number")]
    pub median_ms: u64,
    #[ts(type = "number")]
    pub slowest_ms: u64,
    #[ts(type = "number")]
    pub total_ms: u64,
    /// Whole percent of all tool time. Computed once, here, so a bar drawn
    /// from it and a column printed from it cannot round differently.
    pub share: u32,
    /// Whether this tool ran a whole sub-agent inside itself.
    ///
    /// True for a `spawn`, and it is why this table is not summed against the
    /// turn's own total: the delegate's model calls and tools are inside this
    /// row's duration. Flagged rather than quietly excluded, because the time
    /// is real and somebody looking at a slow turn wants to see it.
    pub nested: bool,
}

/// One step of a turn, as a bar on a timeline.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TraceStep {
    pub kind: SpanKind,
    pub name: String,
    /// Milliseconds after the turn opened. What positions the bar.
    #[ts(type = "number")]
    pub offset_ms: u64,
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// How far below the turn this sits. Delegation is the only thing that
    /// makes it more than one, and seeing that indent is most of how a
    /// delegated turn reads differently from a flat one.
    pub depth: u32,
    pub error: Option<String>,
    /// Output tokens, for a model call that reported them.
    #[ts(type = "number | null")]
    pub output_tokens: Option<u64>,
}

/// One turn, with everything that happened inside it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TurnTrace {
    /// The recorder's own sequence number. Stable for as long as the record is
    /// retained, and unique for the life of the process — which is what a
    /// frontend needs it for, as a key.
    #[ts(type = "number")]
    pub seq: u64,
    /// The conversation it ran in, so a panel can say which.
    pub conversation: String,
    pub model: String,
    pub provider: String,
    /// Unix milliseconds. Formatting is the frontend's business.
    #[ts(type = "number")]
    pub started: u64,
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// Time inside a model call — every `chat` under this turn, including a
    /// delegate's.
    #[ts(type = "number")]
    pub model_ms: u64,
    /// The rest of the wall time: tools doing their own work, the harness
    /// between steps, and waiting. See the module docs for why this is not
    /// called tool time.
    #[ts(type = "number")]
    pub other_ms: u64,
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    pub finish: Option<String>,
    pub error: Option<String>,
    /// Everything under it, earliest first.
    pub steps: Vec<TraceStep>,
}

/// The whole account, ready to print or to draw.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TraceReport {
    /// Turns, not counting a delegate's — those are steps of the turn that
    /// asked for them.
    pub turns: u32,
    /// Every span still retained, of all three kinds.
    pub spans: u32,
    /// How many fell off the end of the ring since the process started.
    ///
    /// Carried so the panel can say the window it is describing has a start.
    /// A dashboard that has quietly stopped covering the period it appears to
    /// cover is worse than one that says so.
    #[ts(type = "number")]
    pub dropped: u64,
    /// Unix milliseconds of the oldest retained span, or `None` if there are
    /// none. What "since" means on the panel.
    #[ts(type = "number | null")]
    pub since: Option<u64>,
    #[ts(type = "number")]
    pub total_ms: u64,
    #[ts(type = "number")]
    pub model_ms: u64,
    #[ts(type = "number")]
    pub other_ms: u64,
    #[ts(type = "number")]
    pub median_turn_ms: u64,
    #[ts(type = "number")]
    pub slowest_turn_ms: u64,
    /// Spans of any kind that ended in an error.
    pub failures: u32,
    /// Slowest first, then by name so the order is stable between reads.
    pub models: Vec<ModelLatency>,
    /// Heaviest first, then by name.
    pub tools: Vec<ToolLatency>,
    /// The most recent turns, newest first. At most [`TURNS_DESCRIBED`].
    pub recent: Vec<TurnTrace>,
}

impl TraceReport {
    /// Whether anything has been recorded at all.
    ///
    /// Its own question rather than `turns == 0` at each call site, because a
    /// process that has run tools but not finished a turn has spans worth
    /// showing and no turns.
    pub fn is_empty(&self) -> bool {
        self.spans == 0
    }
}

/// The account for one conversation, or for everything this process has done.
///
/// `conversation` names a session; `None` is every turn in this process,
/// including delegates' and including conversations that have since been
/// closed. Asking about a conversation with nothing recorded is not an error —
/// it is the ordinary state of one that has not had a turn yet.
pub fn report(traces: &Traces, conversation: Option<&str>) -> TraceReport {
    let snapshot = traces.snapshot();
    let all = snapshot.records;

    // A tool span carries no conversation of its own. It inherits one from the
    // nearest turn above it, which is the only place the id was ever recorded.
    let index: HashMap<u64, usize> = all.iter().enumerate().map(|(i, r)| (r.seq, i)).collect();
    let owners: Vec<Option<String>> = all
        .iter()
        .map(|record| conversation_of(record, &all, &index))
        .collect();

    let kept: Vec<&SpanRecord> = match conversation {
        Some(wanted) => all
            .iter()
            .zip(&owners)
            .filter(|(_, owner)| owner.as_deref() == Some(wanted))
            .map(|(record, _)| record)
            .collect(),
        None => all.iter().collect(),
    };

    if kept.is_empty() {
        // Still worth answering rather than erroring: an empty report with the
        // dropped count in it is how the panel says "nothing here yet".
        return TraceReport {
            dropped: snapshot.dropped,
            ..Default::default()
        };
    }

    // Present in the retained set, which is not the same as having existed. A
    // turn whose parent fell off the end of the ring reads as a root, and that
    // is the right answer: what is left of it *is* the top of what is known.
    let present: HashSet<u64> = kept.iter().map(|r| r.seq).collect();
    let roots: Vec<&SpanRecord> = kept
        .iter()
        .copied()
        .filter(|r| r.kind == SpanKind::Turn && !has_present_ancestor(r, &all, &index, &present))
        .collect();

    let children = children_by_parent(&kept);
    let mut recent: Vec<TurnTrace> = roots.iter().map(|turn| describe(turn, &children)).collect();
    // Newest first, and by sequence for two that opened in the same
    // millisecond — which happens, and an unstable order would make the list
    // reshuffle between reads of the same data.
    recent.sort_by(|a, b| b.started.cmp(&a.started).then(b.seq.cmp(&a.seq)));

    let turn_times: Vec<u64> = recent.iter().map(|t| t.duration_ms).collect();
    TraceReport {
        turns: recent.len() as u32,
        spans: kept.len() as u32,
        dropped: snapshot.dropped,
        since: kept.iter().map(|r| r.started).min(),
        total_ms: turn_times.iter().sum(),
        model_ms: recent.iter().map(|t| t.model_ms).sum(),
        other_ms: recent.iter().map(|t| t.other_ms).sum(),
        median_turn_ms: median(&turn_times),
        slowest_turn_ms: turn_times.iter().copied().max().unwrap_or_default(),
        failures: kept.iter().filter(|r| r.error.is_some()).count() as u32,
        models: models(&kept),
        tools: tools(&kept, &children),
        recent: recent.into_iter().take(TURNS_DESCRIBED).collect(),
    }
}

/// The conversation a record belongs to: the *outermost* one that names it.
///
/// The highest ancestor rather than the nearest, and the difference is
/// delegation. A sub-agent runs in a session of its own and its spans say so,
/// so the nearest answer for a delegate's model call is the delegate's id —
/// which is not a conversation anybody has open, and filing it there would
/// leave the `spawn` on the panel as an opaque block with its contents
/// removed. The turn belongs to whoever ran it, and so does everything that
/// happened inside it.
///
/// The consequence is deliberate: asking about a delegate's own id finds
/// nothing, because that id names work that is filed under the turn above it.
fn conversation_of(
    record: &SpanRecord,
    all: &[SpanRecord],
    index: &HashMap<u64, usize>,
) -> Option<String> {
    let mut current = record;
    let mut found = current.conversation.clone();
    // Bounded by the ring, so a cycle — which cannot happen, because a parent
    // is always an older sequence number — still could not spin forever.
    for _ in 0..all.len() {
        let Some(at) = current.parent.and_then(|seq| index.get(&seq)) else {
            break;
        };
        current = &all[*at];
        if let Some(id) = &current.conversation {
            found = Some(id.clone());
        }
    }
    found
}

/// Whether anything above this record survived in the retained set.
fn has_present_ancestor(
    record: &SpanRecord,
    all: &[SpanRecord],
    index: &HashMap<u64, usize>,
    present: &HashSet<u64>,
) -> bool {
    let mut parent = record.parent;
    for _ in 0..all.len() {
        let Some(seq) = parent else { return false };
        if present.contains(&seq) {
            return true;
        }
        // Named a parent that is gone: the chain ends here rather than
        // continuing, because there is nothing left to follow.
        let Some(at) = index.get(&seq) else {
            return false;
        };
        parent = all[*at].parent;
    }
    false
}

fn children_by_parent<'a>(kept: &[&'a SpanRecord]) -> HashMap<u64, Vec<&'a SpanRecord>> {
    let mut children: HashMap<u64, Vec<&SpanRecord>> = HashMap::new();
    for record in kept {
        if let Some(parent) = record.parent {
            children.entry(parent).or_default().push(record);
        }
    }
    children
}

/// One turn and everything under it, flattened onto a timeline.
fn describe(turn: &SpanRecord, children: &HashMap<u64, Vec<&SpanRecord>>) -> TurnTrace {
    let mut steps = Vec::new();
    collect(turn, children, 1, turn.started, &mut steps);
    steps.sort_by_key(|s| (s.offset_ms, s.duration_ms));

    let model_ms = descendants(turn, children)
        .filter(|r| r.kind == SpanKind::Chat)
        .map(|r| r.duration_ms)
        .sum();

    // The turn's own figures when the provider reported them, and the sum of
    // its model calls when it did not. One source at a time rather than a mix:
    // a turn that failed part-way has no total of its own, and the calls it
    // did make still cost what they cost.
    let (input_tokens, output_tokens) = match (turn.input_tokens, turn.output_tokens) {
        (Some(input), Some(output)) => (input, output),
        _ => {
            let chats = descendants(turn, children).filter(|r| r.kind == SpanKind::Chat);
            chats.fold((0, 0), |(input, output), r| {
                (
                    input + r.input_tokens.unwrap_or_default(),
                    output + r.output_tokens.unwrap_or_default(),
                )
            })
        }
    };

    TurnTrace {
        seq: turn.seq,
        conversation: turn.conversation.clone().unwrap_or_default(),
        model: turn.name.clone(),
        provider: turn.provider.clone().unwrap_or_default(),
        started: turn.started,
        duration_ms: turn.duration_ms,
        model_ms,
        // Saturating, because the two are measured differently — a turn's
        // duration is one monotonic reading and this is a sum of others — and
        // a rounding disagreement must not produce a negative bar.
        other_ms: turn.duration_ms.saturating_sub(model_ms),
        input_tokens,
        output_tokens,
        finish: turn.finish.clone(),
        error: turn.error.clone(),
        steps,
    }
}

fn collect(
    parent: &SpanRecord,
    children: &HashMap<u64, Vec<&SpanRecord>>,
    depth: u32,
    origin: u64,
    out: &mut Vec<TraceStep>,
) {
    for child in children.get(&parent.seq).into_iter().flatten() {
        out.push(TraceStep {
            kind: child.kind,
            name: child.name.clone(),
            offset_ms: child.started.saturating_sub(origin),
            duration_ms: child.duration_ms,
            depth,
            error: child.error.clone(),
            output_tokens: child.output_tokens,
        });
        collect(child, children, depth + 1, origin, out);
    }
}

/// Every span under this one, at any depth.
fn descendants<'a>(
    root: &SpanRecord,
    children: &'a HashMap<u64, Vec<&'a SpanRecord>>,
) -> impl Iterator<Item = &'a SpanRecord> {
    let mut stack: Vec<&SpanRecord> = children.get(&root.seq).into_iter().flatten().copied().collect();
    std::iter::from_fn(move || {
        let next = stack.pop()?;
        stack.extend(children.get(&next.seq).into_iter().flatten().copied());
        Some(next)
    })
}

fn models(kept: &[&SpanRecord]) -> Vec<ModelLatency> {
    let mut grouped: HashMap<(String, String), Vec<&SpanRecord>> = HashMap::new();
    for record in kept.iter().filter(|r| r.kind == SpanKind::Chat) {
        let key = (
            record.name.clone(),
            record.provider.clone().unwrap_or_default(),
        );
        grouped.entry(key).or_default().push(record);
    }

    let mut models: Vec<ModelLatency> = grouped
        .into_iter()
        .map(|((name, provider), calls)| {
            let times: Vec<u64> = calls.iter().map(|r| r.duration_ms).collect();
            let total_ms: u64 = times.iter().sum();
            let output_tokens: u64 = calls.iter().filter_map(|r| r.output_tokens).sum();
            let cached: Vec<u64> = calls.iter().filter_map(|r| r.cached_tokens).collect();
            ModelLatency {
                name,
                provider,
                calls: calls.len() as u32,
                median_ms: median(&times),
                slowest_ms: times.iter().copied().max().unwrap_or_default(),
                input_tokens: calls.iter().filter_map(|r| r.input_tokens).sum(),
                output_tokens,
                // Absent unless some call reported one. Summing `None` as zero
                // would report a backend with no cache as a backend whose
                // cache never hit.
                cached_tokens: (!cached.is_empty()).then(|| cached.iter().sum()),
                failures: calls.iter().filter(|r| r.error.is_some()).count() as u32,
                output_per_second: (total_ms > 0 && output_tokens > 0)
                    .then(|| (output_tokens * 1000 / total_ms) as u32),
            }
        })
        .collect();
    models.sort_by(|a, b| b.slowest_ms.cmp(&a.slowest_ms).then(a.name.cmp(&b.name)));
    models
}

fn tools(kept: &[&SpanRecord], children: &HashMap<u64, Vec<&SpanRecord>>) -> Vec<ToolLatency> {
    let mut grouped: HashMap<String, Vec<&SpanRecord>> = HashMap::new();
    for record in kept.iter().filter(|r| r.kind == SpanKind::Tool) {
        grouped.entry(record.name.clone()).or_default().push(record);
    }

    let mut tools: Vec<ToolLatency> = grouped
        .into_iter()
        .map(|(name, calls)| {
            let times: Vec<u64> = calls.iter().map(|r| r.duration_ms).collect();
            ToolLatency {
                name,
                calls: calls.len() as u32,
                failures: calls.iter().filter(|r| r.error.is_some()).count() as u32,
                median_ms: median(&times),
                slowest_ms: times.iter().copied().max().unwrap_or_default(),
                total_ms: times.iter().sum(),
                share: 0,
                nested: calls.iter().any(|r| children.contains_key(&r.seq)),
            }
        })
        .collect();

    // Shares of this table's own total, settled after the rows are known. See
    // the module docs: this denominator is not the turn's, on purpose.
    let total: u64 = tools.iter().map(|t| t.total_ms).sum();
    if total > 0 {
        for tool in &mut tools {
            tool.share = (tool.total_ms * 100 / total) as u32;
        }
    }
    tools.sort_by(|a, b| b.total_ms.cmp(&a.total_ms).then(a.name.cmp(&b.name)));
    tools
}

/// The middle value, or the lower of the two middles.
///
/// Not an average. One thirty-second command among nine fast ones moves a mean
/// far enough to describe a machine nobody is using, and the whole reason to
/// report a middle *and* a worst is that the pair says what one number cannot.
fn median(times: &[u64]) -> u64 {
    if times.is_empty() {
        return 0;
    }
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    sorted[(sorted.len() - 1) / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record with everything unremarkable, for a test to change one thing.
    fn span(seq: u64, kind: SpanKind, name: &str, started: u64, duration_ms: u64) -> SpanRecord {
        SpanRecord {
            seq,
            parent: None,
            kind,
            name: name.into(),
            provider: Some("ollama".into()),
            conversation: None,
            started,
            duration_ms,
            input_tokens: None,
            output_tokens: None,
            cached_tokens: None,
            finish: None,
            error: None,
        }
    }

    fn turn(seq: u64, conversation: &str, started: u64, duration_ms: u64) -> SpanRecord {
        SpanRecord {
            conversation: Some(conversation.into()),
            ..span(seq, SpanKind::Turn, "qwen3.6:27b", started, duration_ms)
        }
    }

    fn under(parent: u64, record: SpanRecord) -> SpanRecord {
        SpanRecord {
            parent: Some(parent),
            ..record
        }
    }

    /// A ring holding exactly these, in the order children close first.
    fn ring(records: Vec<SpanRecord>) -> Traces {
        let traces = Traces::new();
        for record in records {
            traces.push(record);
        }
        traces
    }

    #[test]
    fn a_turn_splits_into_model_time_and_everything_else() {
        // The one split that is safe to make: model calls never nest inside
        // one another, so summing them is exact, and the remainder is honestly
        // named rather than being called tool time.
        let traces = ring(vec![
            under(1, span(2, SpanKind::Chat, "qwen3.6:27b", 1_000, 600)),
            under(1, span(3, SpanKind::Tool, "read_file", 1_600, 300)),
            turn(1, "s1", 1_000, 1_000),
        ]);

        let report = report(&traces, None);
        assert_eq!(report.turns, 1);
        assert_eq!(report.model_ms, 600);
        assert_eq!(report.other_ms, 400);
        assert_eq!(report.total_ms, 1_000);
    }

    #[test]
    fn a_delegate_does_not_have_its_time_counted_twice() {
        // The reason the headline is model-against-everything-else. A `spawn`
        // holds a whole sub-agent turn, and adding that tool's duration to the
        // durations inside it would produce a turn that spent 180% of itself.
        let traces = ring(vec![
            under(3, span(4, SpanKind::Chat, "qwen3.6:27b", 1_100, 700)),
            under(2, turn(3, "s2", 1_100, 800)),
            under(1, span(2, SpanKind::Tool, "spawn", 1_050, 900)),
            turn(1, "s1", 1_000, 1_000),
        ]);

        let report = report(&traces, None);
        // One turn: the delegate's is a step of the turn that asked for it.
        assert_eq!(report.turns, 1);
        assert_eq!(report.model_ms, 700, "the delegate's model call, once");
        assert_eq!(report.other_ms, 300);
        assert_eq!(report.model_ms + report.other_ms, report.total_ms);

        let spawn = &report.tools[0];
        assert_eq!(spawn.name, "spawn");
        assert!(spawn.nested, "a spawn contains the delegate's whole turn");
    }

    #[test]
    fn a_tool_call_is_filed_under_the_conversation_of_the_turn_above_it() {
        // A tool span carries no conversation of its own — the id was only
        // ever recorded on the turn — so without the walk upward the
        // per-conversation view would show model calls and no tools at all.
        let traces = ring(vec![
            under(1, span(2, SpanKind::Tool, "read_file", 1_000, 50)),
            turn(1, "s1", 1_000, 100),
            under(3, span(4, SpanKind::Tool, "grep", 2_000, 50)),
            turn(3, "s2", 2_000, 100),
        ]);

        let mine = report(&traces, Some("s1"));
        assert_eq!(mine.turns, 1);
        assert_eq!(mine.tools.len(), 1);
        assert_eq!(mine.tools[0].name, "read_file");
    }

    #[test]
    fn a_delegates_spans_are_filed_under_the_turn_that_asked_for_them() {
        // A sub-agent runs in a session of its own and its spans say so. Filed
        // there, the `spawn` on the panel would be an opaque block with its
        // contents removed — and that id is not a conversation anybody has
        // open, so nothing would ever ask for them.
        let traces = ring(vec![
            under(3, span(4, SpanKind::Chat, "qwen3.6:27b", 1_100, 700)),
            under(2, turn(3, "s2", 1_100, 800)),
            under(1, span(2, SpanKind::Tool, "spawn", 1_050, 900)),
            turn(1, "s1", 1_000, 1_000),
        ]);

        let mine = report(&traces, Some("s1"));
        assert_eq!(mine.spans, 4, "the delegate's two spans came along");
        assert_eq!(mine.recent[0].steps.len(), 3);
        assert_eq!(mine.model_ms, 700, "the delegate's model call counts");

        // And the delegate's own id names nothing of its own, which is the
        // other half of the same rule.
        assert!(report(&traces, Some("s2")).is_empty());
    }

    #[test]
    fn a_conversation_with_nothing_recorded_is_an_empty_report_and_not_an_error() {
        // The ordinary state of a conversation that has not had a turn yet.
        let traces = ring(vec![turn(1, "s1", 1_000, 100)]);
        let report = report(&traces, Some("s2"));
        assert!(report.is_empty());
        assert_eq!(report.turns, 0);
    }

    #[test]
    fn a_turn_whose_parent_fell_off_the_ring_reads_as_a_turn() {
        // What is left of it *is* the top of what is known. Treating it as a
        // nested step would hide it from the list entirely, and it is the
        // oldest thing there — the most likely to be half-remembered.
        let traces = ring(vec![under(999, turn(1, "s1", 1_000, 100))]);
        assert_eq!(report(&traces, None).turns, 1);
    }

    #[test]
    fn the_middle_call_is_reported_rather_than_the_average() {
        // One thirty-second command among fast ones moves a mean far enough to
        // describe a machine nobody is using.
        let traces = ring(vec![
            under(1, span(2, SpanKind::Tool, "run_command", 1_000, 10)),
            under(1, span(3, SpanKind::Tool, "run_command", 1_010, 20)),
            under(1, span(4, SpanKind::Tool, "run_command", 1_030, 30_000)),
            turn(1, "s1", 1_000, 31_000),
        ]);

        let tool = &report(&traces, None).tools[0];
        assert_eq!(tool.median_ms, 20);
        assert_eq!(tool.slowest_ms, 30_000);
        assert_eq!(tool.calls, 3);
    }

    #[test]
    fn a_backend_with_no_cache_reports_no_cache_rather_than_a_miss() {
        // The same rule the usage account keeps: absent and zero are different
        // facts, and a 0% hit rate beside a local model is a wrong answer to a
        // question it cannot be asked.
        let traces = ring(vec![
            under(1, span(2, SpanKind::Chat, "qwen3.6:27b", 1_000, 500)),
            turn(1, "s1", 1_000, 600),
        ]);
        assert_eq!(report(&traces, None).models[0].cached_tokens, None);

        let traces = ring(vec![
            under(
                1,
                SpanRecord {
                    cached_tokens: Some(1_024),
                    input_tokens: Some(2_048),
                    ..span(2, SpanKind::Chat, "qwen3.6:27b", 1_000, 500)
                },
            ),
            turn(1, "s1", 1_000, 600),
        ]);
        assert_eq!(report(&traces, None).models[0].cached_tokens, Some(1_024));
    }

    #[test]
    fn a_models_throughput_is_output_tokens_over_the_time_it_took() {
        let traces = ring(vec![
            under(
                1,
                SpanRecord {
                    output_tokens: Some(100),
                    ..span(2, SpanKind::Chat, "qwen3.6:27b", 1_000, 2_000)
                },
            ),
            turn(1, "s1", 1_000, 2_100),
        ]);
        assert_eq!(report(&traces, None).models[0].output_per_second, Some(50));
    }

    #[test]
    fn the_steps_of_a_turn_are_placed_against_its_own_start() {
        // A waterfall is drawn from offsets. Absolute timestamps would make
        // every bar start at the far right of a millisecond-since-1970 scale.
        let traces = ring(vec![
            under(2, span(3, SpanKind::Chat, "qwen3.6:27b", 1_400, 100)),
            under(1, span(2, SpanKind::Tool, "spawn", 1_300, 400)),
            under(1, span(4, SpanKind::Chat, "qwen3.6:27b", 1_000, 200)),
            turn(1, "s1", 1_000, 800),
        ]);

        let steps = &report(&traces, None).recent[0].steps;
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].offset_ms, 0, "earliest first");
        assert_eq!(steps[0].depth, 1);
        assert_eq!(steps[1].name, "spawn");
        assert_eq!(steps[1].offset_ms, 300);
        assert_eq!(steps[2].offset_ms, 400);
        assert_eq!(steps[2].depth, 2, "nested under the spawn");
    }

    #[test]
    fn the_newest_turn_is_first() {
        let traces = ring(vec![
            turn(1, "s1", 1_000, 100),
            turn(2, "s1", 5_000, 100),
            turn(3, "s1", 3_000, 100),
        ]);
        let seqs: Vec<u64> = report(&traces, None).recent.iter().map(|t| t.seq).collect();
        assert_eq!(seqs, vec![2, 3, 1]);
    }

    #[test]
    fn a_turn_falls_back_to_its_model_calls_when_it_reported_no_total() {
        // A turn that failed part-way has no usage of its own, and the calls
        // it did make still cost what they cost.
        let traces = ring(vec![
            under(
                1,
                SpanRecord {
                    input_tokens: Some(900),
                    output_tokens: Some(40),
                    ..span(2, SpanKind::Chat, "qwen3.6:27b", 1_000, 500)
                },
            ),
            SpanRecord {
                error: Some("provider".into()),
                ..turn(1, "s1", 1_000, 600)
            },
        ]);

        let turn = &report(&traces, None).recent[0];
        assert_eq!(turn.input_tokens, 900);
        assert_eq!(turn.output_tokens, 40);
        assert_eq!(turn.error.as_deref(), Some("provider"));
    }

    #[test]
    fn a_full_ring_is_reported_as_one_that_has_forgotten_things() {
        // The panel has to be able to say the window has a start. Without this
        // it would describe six hours of work as "everything since launch".
        let traces = Traces::new();
        for seq in 0..(taurus_core::telemetry::store::CAPACITY as u64 + 5) {
            traces.push(turn(seq + 1, "s1", 1_000 + seq, 10));
        }
        assert_eq!(report(&traces, None).dropped, 5);
    }

    #[test]
    fn nothing_recorded_at_all_still_answers() {
        let report = report(&Traces::new(), None);
        assert!(report.is_empty());
        assert_eq!(report.spans, 0);
        assert_eq!(report.dropped, 0);
        assert!(report.recent.is_empty());
    }
}
