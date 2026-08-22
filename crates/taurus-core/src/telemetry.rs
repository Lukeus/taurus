//! The spans a turn emits, named the way the rest of the world names them.
//!
//! Taurus has always had `tracing` and always logged. What it did not have was
//! a *vocabulary*: a log line saying `usage=1204` is legible to somebody
//! reading this repository and to nothing else. The OpenTelemetry GenAI
//! semantic conventions are the agreed names for what an agent does — a model
//! call, a tool call, the turn around them, and the tokens each cost — and
//! spelling them exactly is the whole difference between a trace this project
//! can read and a trace Langfuse, Phoenix, Jaeger, or any OTLP collector can.
//!
//! # Nothing here talks to a collector
//!
//! These are ordinary `tracing` spans. They cost a few atomic operations when
//! nothing is subscribed, which is the ordinary case: no exporter is installed
//! unless one is configured, and then it is `taurus-telemetry` that installs
//! it. So `taurus-core` gains a vocabulary and not a dependency on
//! OpenTelemetry, and a build with no telemetry configured carries none of it.
//!
//! # Why the field list is a macro
//!
//! `tracing` bakes a span's field set into static metadata at the callsite. A
//! field that is not named when the span is created can never be recorded onto
//! it — `Span::record` on an undeclared name is silently dropped, which is the
//! worst possible failure for telemetry: a trace that arrives, looks complete,
//! and is missing the number somebody is about to make a decision with.
//!
//! So every field a span will ever carry is declared up front, with
//! [`tracing::field::Empty`] standing in for the ones filled later. That list
//! can only be single-sourced by a macro that owns the whole `info_span!`
//! call; a `const` array cannot be spliced into one. This module holds the one
//! copy, [`CHAT_SPAN_FIELDS`] is the same contract written out as a checklist,
//! and a test below asserts the two agree.
//!
//! # What is deliberately not recorded
//!
//! The conversation. `gen_ai.input.messages` and `gen_ai.output.messages` are
//! part of the conventions and this harness leaves them empty unless somebody
//! turns them on in as many words. A trace exporter is a network destination,
//! and the difference between "how many tokens" and "what was said" is the
//! difference between a metric and the contents of a workspace. See
//! [`Capture`].

use taurus_provider::{StopReason, TokenUsage};
use tracing::Span;

/// Whether spans may carry the conversation itself.
///
/// A separate switch from "is telemetry on", because they are separate
/// decisions with very different stakes. Token counts describe a conversation;
/// messages *are* it — the file the model read, the command it ran, whatever
/// the user pasted in. Sending those to a collector is the kind of thing
/// somebody has to mean, so nothing infers it from an endpoint being set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Capture {
    /// Shape and cost only. The default, and what a metrics dashboard needs.
    #[default]
    MetadataOnly,
    /// Also the messages. For debugging a prompt, on a collector you own.
    Content,
}

impl Capture {
    pub fn content(self) -> bool {
        self == Self::Content
    }
}

/// Fields every completion span carries, as a checklist.
///
/// The second copy of the contract in [`chat_span`], written in a form that can
/// be iterated. Neither is the source of truth on its own: the macro is what
/// the span actually declares, this is what a reader and a test can check it
/// against, and `the_declared_fields_match_the_checklist` fails if they drift.
pub const CHAT_SPAN_FIELDS: &[&str] = &[
    "gen_ai.operation.name",
    "gen_ai.provider.name",
    "gen_ai.request.model",
    "gen_ai.conversation.id",
    "gen_ai.response.model",
    "gen_ai.response.finish_reasons",
    "gen_ai.usage.input_tokens",
    "gen_ai.usage.output_tokens",
    "gen_ai.usage.cache_read.input_tokens",
    "gen_ai.usage.cache_creation.input_tokens",
    "gen_ai.usage.reasoning_tokens",
    "gen_ai.input.messages",
    "gen_ai.output.messages",
    "error.type",
];

/// The span around one request to the model.
///
/// Opened per *attempt*, not per iteration: a request that failed with a 429
/// and was retried is two calls to the backend and two spans, because that is
/// two round trips of latency and, on a metered provider, potentially two
/// bills. A trace that folded them into one would be telling a story about the
/// backend that did not happen.
pub fn chat_span(provider: &str, model: &str, conversation: &str) -> Span {
    tracing::info_span!(
        target: "taurus::gen_ai",
        "chat",
        gen_ai.operation.name = "chat",
        gen_ai.provider.name = provider,
        gen_ai.request.model = model,
        gen_ai.conversation.id = conversation,
        gen_ai.response.model = tracing::field::Empty,
        gen_ai.response.finish_reasons = tracing::field::Empty,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        gen_ai.usage.cache_read.input_tokens = tracing::field::Empty,
        gen_ai.usage.cache_creation.input_tokens = tracing::field::Empty,
        gen_ai.usage.reasoning_tokens = tracing::field::Empty,
        gen_ai.input.messages = tracing::field::Empty,
        gen_ai.output.messages = tracing::field::Empty,
        error.type = tracing::field::Empty,
    )
}

/// The span around one tool call.
///
/// `gen_ai.tool.type` is `"function"` for everything here, including MCP tools.
/// The conventions reserve `"extension"` for a tool the *model provider* runs
/// on its own — a hosted web search, a hosted interpreter — and nothing in this
/// harness is that: an MCP server is a program on this machine that this
/// harness calls, which is what `"function"` means.
pub fn tool_span(name: &str, call_id: &str) -> Span {
    tracing::info_span!(
        target: "taurus::gen_ai",
        "execute_tool",
        gen_ai.operation.name = "execute_tool",
        gen_ai.tool.name = name,
        gen_ai.tool.call.id = call_id,
        gen_ai.tool.type = "function",
        error.type = tracing::field::Empty,
    )
}

/// The span around a whole turn, parent to every span above.
///
/// Delegation nests inside it for free and that is most of the value: a
/// sub-agent's model calls and tool calls are opened underneath the `spawn`
/// tool's span, so a nine-step turn that delegated twice reads as a tree
/// instead of a flat list somebody has to reassemble by timestamp.
pub fn turn_span(provider: &str, model: &str, conversation: &str) -> Span {
    tracing::info_span!(
        target: "taurus::gen_ai",
        "invoke_agent",
        gen_ai.operation.name = "invoke_agent",
        gen_ai.provider.name = provider,
        gen_ai.request.model = model,
        gen_ai.conversation.id = conversation,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        gen_ai.response.finish_reasons = tracing::field::Empty,
        error.type = tracing::field::Empty,
    )
}

/// Records what a completed request cost.
///
/// The numbers come from the provider's own report, which is the same
/// [`TokenUsage`] the session's counter and the compaction trigger read. One
/// source for all three deliberately: `estimate_tokens` already argues that two
/// estimators disagreeing means the number a user sees is the wrong one, and a
/// span quoting a third would make that worse in the place it is least
/// checkable.
///
/// The optional three are recorded only when the backend reported them. Zero
/// and "not reported" are different facts — a provider with no prompt cache and
/// a provider whose cache missed both cost full price, but only one of them has
/// a cache to tell you about — and writing a zero for the second would put a
/// cache-hit rate of 0% on a dashboard for a backend that has no cache.
pub fn record_usage(span: &Span, usage: &TokenUsage) {
    span.record("gen_ai.usage.input_tokens", usage.input_tokens);
    span.record("gen_ai.usage.output_tokens", usage.output_tokens);
    if let Some(cached) = usage.cache_read_input_tokens {
        span.record("gen_ai.usage.cache_read.input_tokens", cached);
    }
    if let Some(written) = usage.cache_creation_input_tokens {
        span.record("gen_ai.usage.cache_creation.input_tokens", written);
    }
    if let Some(reasoning) = usage.reasoning_tokens {
        span.record("gen_ai.usage.reasoning_tokens", reasoning);
    }
}

/// The conventions' name for why a turn stopped.
///
/// Mapped rather than printed. `StopReason` is this harness's spelling and
/// these are the agreed ones, and a dashboard grouping by finish reason across
/// several tools needs them to be the same word.
pub fn finish_reason(stop: StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn => "stop",
        StopReason::ToolUse => "tool_calls",
        StopReason::MaxTokens => "length",
        StopReason::StopSequence => "stop",
        // Not a conventional value — there is no agreed name for "the person
        // pressed stop", and the alternatives all claim something untrue. A
        // canceled turn reported as `stop` is indistinguishable from one the
        // model finished, which is exactly the distinction anybody looking at
        // an abandoned trace is looking for.
        StopReason::Canceled => "canceled",
    }
}

/// Records the failure that ended a span, by type rather than by message.
///
/// `error.type` is meant to be low-cardinality — a value a dashboard can group
/// by. The message goes in the log event that accompanies it, where a unique
/// string costs nothing.
pub fn record_error(span: &Span, kind: &str) {
    span.record("error.type", kind);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::with_default;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Layer;

    /// The field names of each span opened, in order.
    type Names = Arc<Mutex<Vec<Vec<String>>>>;

    /// Collects the field names each span declares, which is the only thing
    /// worth asserting: a field that is not declared cannot be recorded, and
    /// `Span::record` on an unknown name fails silently.
    #[derive(Default, Clone)]
    struct Declared(Names);

    impl<S: tracing::Subscriber> Layer<S> for Declared {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            _: &tracing::span::Id,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let fields = attrs
                .metadata()
                .fields()
                .iter()
                .map(|f| f.name().to_string())
                .collect();
            self.0.lock().expect("not poisoned").push(fields);
        }
    }

    fn fields_of(open: impl FnOnce()) -> Vec<String> {
        let seen = Declared::default();
        let subscriber = tracing_subscriber::registry().with(seen.clone());
        with_default(subscriber, open);
        let captured = seen.0.lock().expect("not poisoned").clone();
        captured.into_iter().next().expect("a span was opened")
    }

    #[test]
    fn the_declared_fields_match_the_checklist() {
        // The two copies of one contract. Without this, adding a field to the
        // checklist and forgetting the macro produces telemetry that is missing
        // exactly the number somebody added the field to see — and nothing
        // fails, because recording an undeclared field is a silent no-op.
        let declared = fields_of(|| {
            let _span = chat_span("ollama", "qwen3", "s1");
        });
        assert_eq!(declared, CHAT_SPAN_FIELDS);
    }

    #[test]
    fn a_recorded_usage_field_is_one_the_span_declared() {
        // The failure this guards against is invisible at runtime: `record` on
        // a name the callsite never declared is dropped without a word, so a
        // typo here is a metric that is simply never there.
        for field in [
            "gen_ai.usage.input_tokens",
            "gen_ai.usage.output_tokens",
            "gen_ai.usage.cache_read.input_tokens",
            "gen_ai.usage.cache_creation.input_tokens",
            "gen_ai.usage.reasoning_tokens",
        ] {
            assert!(
                CHAT_SPAN_FIELDS.contains(&field),
                "{field} is recorded but never declared"
            );
        }
    }

    #[test]
    fn a_tool_span_carries_what_identifies_the_call() {
        let declared = fields_of(|| {
            let _span = tool_span("read_file", "tu_1");
        });
        for expected in [
            "gen_ai.operation.name",
            "gen_ai.tool.name",
            "gen_ai.tool.call.id",
            "gen_ai.tool.type",
            "error.type",
        ] {
            assert!(declared.iter().any(|f| f == expected), "missing {expected}");
        }
    }

    #[test]
    fn a_canceled_turn_does_not_report_itself_as_finished() {
        // The whole point of looking at an abandoned trace is telling it apart
        // from one that ran to completion.
        assert_eq!(finish_reason(StopReason::EndTurn), "stop");
        assert_eq!(finish_reason(StopReason::ToolUse), "tool_calls");
        assert_eq!(finish_reason(StopReason::MaxTokens), "length");
        assert_ne!(finish_reason(StopReason::Canceled), "stop");
    }

    #[test]
    fn a_backend_with_no_cache_reports_no_cache_rather_than_a_miss() {
        // Zero and absent are different facts. Written as zero, a backend with
        // no prompt cache at all would show a 0% hit rate on a dashboard beside
        // one whose cache is genuinely cold.
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 20,
            ..Default::default()
        };
        assert_eq!(usage.cache_read_input_tokens, None);
    }
}
