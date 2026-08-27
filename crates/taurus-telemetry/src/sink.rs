//! The layer that keeps finished spans in this process.
//!
//! [`taurus_core::telemetry::store`] argues for why the ring exists at all —
//! that a collector is the right place for history and the wrong thing to ask
//! somebody to stand up before they can find out why one turn was slow. This
//! is the part that fills it.
//!
//! It is in this crate rather than beside the ring for one reason:
//! `tracing-subscriber` is here already, and `taurus-core` should not grow a
//! dependency on the subscriber machinery to hold a `Vec` of structs. Core
//! keeps the vocabulary and the buffer; the crate that already owns the
//! subscriber owns the thing that writes into it.
//!
//! # Why open-to-close and not entered time
//!
//! A span's *busy* time — the sum of the stretches it was actually on a thread
//! — is the honest number for CPU work and the misleading one for this. Almost
//! everything a turn does is waiting: on a model streaming tokens back, on a
//! command running, on a network that is slow today. Busy time reports a
//! ninety-second turn as four milliseconds of work, which is true and answers
//! nobody's question. Wall time from open to close is the ninety seconds, and
//! that is what somebody opened the panel about.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use taurus_core::telemetry::{SpanKind, SpanRecord, Traces};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// The target every span this keeps is emitted on.
///
/// The same one the exporter filters to, and for the same reason: without it
/// the ring fills with reqwest, hyper, and rustls internals — every one of them
/// a real span, and none of them the thing anybody opened the panel to look at.
/// Checked here rather than with a `Targets` filter so that what is kept is
/// decided in one visible place, next to the code that decides what is kept
/// *about* it.
const TARGET: &str = "taurus::gen_ai";

/// Writes every finished harness span into a [`Traces`] ring.
pub struct Recorder {
    traces: Traces,
    /// The next sequence number. Handed out once each and never reused, which
    /// is the whole reason a record does not simply carry the subscriber's
    /// span id — see [`taurus_core::telemetry::store`].
    next: AtomicU64,
}

impl Recorder {
    pub fn new(traces: Traces) -> Self {
        Self {
            traces,
            next: AtomicU64::new(1),
        }
    }
}

impl<S> Layer<S> for Recorder
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(kind) = kind_of(attrs.metadata()) else {
            return;
        };
        let Some(span) = ctx.span(id) else {
            return;
        };

        // The nearest *recorded* ancestor rather than the immediate parent. A
        // span from somewhere else in the middle — a library's, or one of this
        // harness's own outside the gen_ai target — would otherwise break the
        // chain and orphan a tool call from the turn that made it.
        let parent = span.parent().and_then(|p| {
            p.scope()
                .find_map(|a| a.extensions().get::<Pending>().map(|pending| pending.seq))
        });

        let mut fields = Fields::default();
        attrs.record(&mut fields);

        span.extensions_mut().insert(Pending {
            seq: self.next.fetch_add(1, Ordering::Relaxed),
            parent,
            kind,
            opened: Instant::now(),
            started: now_ms(),
            fields,
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        // Absent for every span this does not keep, which is most of them.
        let Some(pending) = extensions.get_mut::<Pending>() else {
            return;
        };
        values.record(&mut pending.fields);
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(pending) = span.extensions_mut().remove::<Pending>() else {
            return;
        };
        self.traces.push(pending.finish());
    }
}

/// Which span this is, by the name the conventions gave it.
///
/// Matched on the span's name rather than on its fields because the name is
/// static metadata: it cannot have been recorded late, and it cannot be
/// missing. Anything else on the target is not one of the three and is not
/// kept — a new span added later has to be named here deliberately, which is
/// the right amount of friction for something that decides what a dashboard
/// counts.
fn kind_of(metadata: &Metadata<'_>) -> Option<SpanKind> {
    if metadata.target() != TARGET {
        return None;
    }
    match metadata.name() {
        "invoke_agent" => Some(SpanKind::Turn),
        "chat" => Some(SpanKind::Chat),
        "execute_tool" => Some(SpanKind::Tool),
        _ => None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

/// A span that has been opened and has not closed yet.
///
/// Lives in the span's own extensions rather than in a map keyed by id, which
/// is what makes it safe: the registry drops it with the span, so a turn that
/// ends in a panic cannot leave an entry behind to leak or, worse, to be
/// picked up by the next span that reuses the id.
struct Pending {
    seq: u64,
    parent: Option<u64>,
    kind: SpanKind,
    opened: Instant,
    started: u64,
    fields: Fields,
}

impl Pending {
    fn finish(self) -> SpanRecord {
        let Fields {
            request_model,
            response_model,
            tool_name,
            provider,
            conversation,
            input_tokens,
            output_tokens,
            cached_tokens,
            finish,
            error,
        } = self.fields;

        // What a reader scans the column for, which is a different thing per
        // kind: a tool call is its name, and a model call is the model that
        // answered — the one the provider reported, when it said, because a
        // backend serving an alias answers as something else and that
        // difference is worth seeing rather than hiding.
        let name = match self.kind {
            SpanKind::Tool => tool_name,
            SpanKind::Chat | SpanKind::Turn => response_model.or(request_model),
        }
        .unwrap_or_else(|| "unknown".to_string());

        SpanRecord {
            seq: self.seq,
            parent: self.parent,
            kind: self.kind,
            name,
            provider,
            conversation,
            started: self.started,
            duration_ms: self.opened.elapsed().as_millis() as u64,
            input_tokens,
            output_tokens,
            cached_tokens,
            finish,
            error,
        }
    }
}

/// The fields worth keeping, filled as they arrive.
///
/// Most of them arrive twice over: declared empty when the span opens and
/// recorded onto when the answer is known. Nothing here distinguishes the two
/// — a later value simply replaces an earlier one, which is what recording
/// onto a span means.
///
/// The two message fields are deliberately absent. They are the conversation
/// itself, and this buffer is read by a panel rather than by a person who has
/// opted into carrying it — see [`taurus_core::telemetry::Capture`].
#[derive(Default)]
struct Fields {
    request_model: Option<String>,
    response_model: Option<String>,
    tool_name: Option<String>,
    provider: Option<String>,
    conversation: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_tokens: Option<u64>,
    finish: Option<String>,
    error: Option<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        let slot = match field.name() {
            "gen_ai.request.model" => &mut self.request_model,
            "gen_ai.response.model" => &mut self.response_model,
            "gen_ai.tool.name" => &mut self.tool_name,
            "gen_ai.provider.name" => &mut self.provider,
            "gen_ai.conversation.id" => &mut self.conversation,
            "gen_ai.response.finish_reasons" => &mut self.finish,
            "error.type" => &mut self.error,
            _ => return,
        };
        *slot = Some(value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let slot = match field.name() {
            "gen_ai.usage.input_tokens" => &mut self.input_tokens,
            "gen_ai.usage.output_tokens" => &mut self.output_tokens,
            "gen_ai.usage.cache_read.input_tokens" => &mut self.cached_tokens,
            _ => return,
        };
        *slot = Some(value);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        // A token count is never negative, and a negative one is a bug
        // somewhere upstream rather than a number to put on a dashboard.
        if let Ok(value) = u64::try_from(value) {
            self.record_u64(field, value);
        }
    }

    /// Required by the trait, and the right thing to do with everything else.
    ///
    /// The fields this keeps are all strings and integers, recorded as such.
    /// Anything that arrives as a `Debug` — the two message fields, or a field
    /// added later and not named above — is not something this buffer has a
    /// column for, and formatting it to throw it away would be work done for
    /// nothing on every span.
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_core::telemetry;
    use taurus_provider::{StopReason, TokenUsage};
    use tracing::subscriber::with_default;
    use tracing_subscriber::layer::SubscriberExt;

    /// Runs `open` with a recorder installed, and hands back what it kept.
    ///
    /// A local subscriber rather than the global one: `install` can only be
    /// called once in a process, and these are several tests.
    fn recorded(open: impl FnOnce()) -> Vec<SpanRecord> {
        let traces = Traces::new();
        let subscriber =
            tracing_subscriber::registry().with(Recorder::new(traces.clone()));
        with_default(subscriber, open);
        traces.snapshot().records
    }

    #[test]
    fn a_turn_keeps_what_it_cost() {
        let records = recorded(|| {
            let span = telemetry::turn_span("ollama", "qwen3.6:27b", "s1");
            let entered = span.enter();
            telemetry::record_usage(
                &span,
                &TokenUsage {
                    input_tokens: 1204,
                    output_tokens: 88,
                    cache_read_input_tokens: Some(1024),
                    ..Default::default()
                },
            );
            span.record(
                "gen_ai.response.finish_reasons",
                telemetry::finish_reason(StopReason::EndTurn),
            );
            drop(entered);
        });

        assert_eq!(records.len(), 1);
        let turn = &records[0];
        assert_eq!(turn.kind, SpanKind::Turn);
        assert_eq!(turn.name, "qwen3.6:27b");
        assert_eq!(turn.provider.as_deref(), Some("ollama"));
        assert_eq!(turn.conversation.as_deref(), Some("s1"));
        assert_eq!(turn.input_tokens, Some(1204));
        assert_eq!(turn.output_tokens, Some(88));
        assert_eq!(turn.cached_tokens, Some(1024));
        assert_eq!(turn.finish.as_deref(), Some("stop"));
        assert_eq!(turn.error, None);
    }

    #[test]
    fn a_tool_call_is_recorded_under_the_turn_that_made_it() {
        // The parent link is the whole feature. Without it a waterfall is a
        // flat list somebody reassembles by timestamp, which is exactly what
        // the trace was supposed to replace.
        let records = recorded(|| {
            let turn = telemetry::turn_span("ollama", "qwen3.6:27b", "s1");
            let _entered = turn.enter();
            let tool = telemetry::tool_span("read_file", "tu_1");
            drop(tool.enter());
        });

        // Children close first: the tool span is dropped inside the turn.
        assert_eq!(records.len(), 2);
        let (tool, turn) = (&records[0], &records[1]);
        assert_eq!(tool.kind, SpanKind::Tool);
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.parent, Some(turn.seq));
        assert_eq!(turn.parent, None);
    }

    #[test]
    fn a_model_call_reports_the_model_that_answered() {
        // A backend serving an alias answers as something else, and that
        // difference is worth seeing rather than hiding behind what was asked
        // for.
        let records = recorded(|| {
            let chat = telemetry::chat_span("ollama", "qwen3.6", "s1");
            let _entered = chat.enter();
            chat.record("gen_ai.response.model", "qwen3.6:27b-instruct");
        });

        assert_eq!(records[0].kind, SpanKind::Chat);
        assert_eq!(records[0].name, "qwen3.6:27b-instruct");
    }

    #[test]
    fn a_failure_is_kept_by_type() {
        let records = recorded(|| {
            let tool = telemetry::tool_span("read_file", "tu_1");
            let _entered = tool.enter();
            telemetry::record_error(&tool, "not_found");
        });

        assert_eq!(records[0].error.as_deref(), Some("not_found"));
    }

    #[test]
    fn a_backend_that_reported_no_cache_is_not_recorded_as_a_miss() {
        // Zero and absent are different facts, all the way through: a record
        // that wrote 0 here would put a 0% hit rate on the panel for a backend
        // that has no cache at all.
        let records = recorded(|| {
            let chat = telemetry::chat_span("ollama", "qwen3.6:27b", "s1");
            let _entered = chat.enter();
            telemetry::record_usage(
                &chat,
                &TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    ..Default::default()
                },
            );
        });

        assert_eq!(records[0].cached_tokens, None);
    }

    #[test]
    fn spans_from_anywhere_else_are_not_kept() {
        // Without the target check the ring fills with reqwest, hyper, and
        // rustls internals, and the turn somebody opened the panel to find is
        // three hundred rows down.
        let records = recorded(|| {
            let _outer = tracing::info_span!("some_library_span").entered();
            let _inner = tracing::info_span!(target: "taurus::gen_ai", "chat").entered();
        });

        // The `chat` above declares none of the fields the real one does,
        // which is the other half of the check: a span kept by name alone
        // still has to survive having nothing in it.
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "unknown");
    }

    #[test]
    fn a_delegate_nests_under_the_tool_that_spawned_it() {
        // Free, and most of the value: a sub-agent's model calls open inside
        // the `spawn` tool's span, so the tree already says who asked for
        // what.
        let records = recorded(|| {
            let turn = telemetry::turn_span("ollama", "qwen3.6:27b", "s1");
            let _turn = turn.enter();
            let spawn = telemetry::tool_span("spawn", "tu_1");
            let _spawn = spawn.enter();
            let inner = telemetry::turn_span("ollama", "qwen3.6:27b", "s2");
            drop(inner.enter());
        });

        // Found by conversation rather than by model: both turns ran the same
        // one, and the delegate's is the record that closed first.
        let find = |conversation: &str| {
            records
                .iter()
                .find(|r| r.kind == SpanKind::Turn && r.conversation.as_deref() == Some(conversation))
                .expect("recorded")
        };
        let outer = find("s1");
        let inner = find("s2");
        let spawn = records
            .iter()
            .find(|r| r.kind == SpanKind::Tool)
            .expect("the spawn");

        assert_eq!(spawn.parent, Some(outer.seq));
        assert_eq!(inner.parent, Some(spawn.seq));
    }
}
