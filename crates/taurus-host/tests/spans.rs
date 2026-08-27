//! A real turn's spans, all the way to the report.
//!
//! Three pieces meet here and nothing else exercises the joins between them:
//! the span vocabulary in `taurus_core::telemetry`, the recorder in
//! `taurus_telemetry::sink` that keeps what those spans said, and
//! `taurus_host::traces`, which does the arithmetic. Each has its own unit
//! tests over hand-built inputs, which is exactly the shape of test that
//! cannot catch a field name that does not match, a parent link that does not
//! form, or a duration that is never measured.
//!
//! So this opens the spans the way `taurus_core::agent` opens them — a turn,
//! model calls inside it, a tool call, a delegate under a `spawn` — and asks
//! the report the questions the panel asks.

use std::time::Duration;

use taurus_core::telemetry::{self, Traces};
use taurus_provider::{StopReason, TokenUsage};
use taurus_telemetry::sink::Recorder;
use tracing::subscriber::with_default;
use tracing_subscriber::layer::SubscriberExt;

/// Long enough to be a duration and short enough not to be felt.
const TICK: Duration = Duration::from_millis(12);

fn usage(input: u32, output: u32) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_input_tokens: Some(input / 2),
        ..Default::default()
    }
}

/// One turn, opened the way the agent loop opens one.
fn run_a_turn(traces: &Traces) {
    let subscriber = tracing_subscriber::registry().with(Recorder::new(traces.clone()));
    with_default(subscriber, || {
        let turn = telemetry::turn_span("ollama", "qwen3.6:27b", "s1");
        let _turn = turn.enter();

        {
            let chat = telemetry::chat_span("ollama", "qwen3.6:27b", "s1");
            let _chat = chat.enter();
            std::thread::sleep(TICK);
            chat.record("gen_ai.response.model", "qwen3.6:27b");
            telemetry::record_usage(&chat, &usage(1_000, 40));
            chat.record(
                "gen_ai.response.finish_reasons",
                telemetry::finish_reason(StopReason::ToolUse),
            );
        }

        {
            let tool = telemetry::tool_span("read_file", "tu_1");
            let _tool = tool.enter();
            std::thread::sleep(TICK);
            telemetry::record_error(&tool, "not_found");
        }

        {
            // A delegate: its whole turn runs inside the spawning tool's span,
            // which is the nesting the report has to see without being told.
            let spawn = telemetry::tool_span("spawn", "tu_2");
            let _spawn = spawn.enter();
            let inner = telemetry::turn_span("ollama", "qwen3.6:27b", "s2");
            let _inner = inner.enter();
            let chat = telemetry::chat_span("ollama", "qwen3.6:27b", "s2");
            let _chat = chat.enter();
            std::thread::sleep(TICK);
            telemetry::record_usage(&chat, &usage(400, 20));
        }

        telemetry::record_usage(&turn, &usage(1_400, 60));
        turn.record(
            "gen_ai.response.finish_reasons",
            telemetry::finish_reason(StopReason::EndTurn),
        );
    });
}

#[test]
fn a_turn_that_delegated_reads_as_one_turn_with_the_delegate_inside_it() {
    let traces = Traces::new();
    run_a_turn(&traces);
    let report = taurus_host::traces::report(&traces, None);

    // One turn, not two. The delegate's is a step of the turn that asked for
    // it, and counting it separately would double every total on the panel.
    assert_eq!(report.turns, 1, "the delegate is not a turn of its own");
    assert_eq!(report.recent.len(), 1);

    let turn = &report.recent[0];
    assert_eq!(turn.conversation, "s1");
    assert_eq!(turn.model, "qwen3.6:27b");
    assert_eq!(turn.provider, "ollama");
    assert_eq!(turn.finish.as_deref(), Some("stop"));
    assert_eq!(turn.input_tokens, 1_400, "the turn's own report, not a sum");

    // Five spans under the turn: two model calls, two tool calls, and the
    // delegate's turn. The delegate's chat is one of the two.
    assert_eq!(turn.steps.len(), 5, "{:#?}", turn.steps);
    let spawn = turn
        .steps
        .iter()
        .find(|s| s.name == "spawn")
        .expect("the spawn is a step");
    let delegate_chat = turn
        .steps
        .iter()
        .find(|s| s.depth == 3)
        .expect("the delegate's model call, two levels down");
    assert_eq!(spawn.depth, 1);
    assert_eq!(delegate_chat.kind, taurus_core::telemetry::SpanKind::Chat);

    // The failure survives the trip by type, which is what a dashboard groups
    // by. The message never enters a span at all.
    let failed = turn
        .steps
        .iter()
        .find(|s| s.error.is_some())
        .expect("the read that failed");
    assert_eq!(failed.name, "read_file");
    assert_eq!(failed.error.as_deref(), Some("not_found"));
    assert_eq!(report.failures, 1);
}

#[test]
fn the_time_a_turn_took_is_measured_rather_than_declared() {
    // Every duration on the panel comes from the gap between a span opening
    // and closing. Nothing here asserts a number — the point is that the
    // measurement happens at all, and that the parts are inside the whole.
    let traces = Traces::new();
    run_a_turn(&traces);
    let report = taurus_host::traces::report(&traces, None);
    let turn = &report.recent[0];

    assert!(
        turn.duration_ms >= TICK.as_millis() as u64 * 3,
        "a turn holding three sleeps lasted {}ms",
        turn.duration_ms
    );
    assert!(turn.model_ms > 0, "two model calls took no time at all");
    assert!(
        turn.model_ms <= turn.duration_ms,
        "the model calls inside a turn cannot outlast it"
    );
    assert_eq!(turn.model_ms + turn.other_ms, turn.duration_ms);
    assert!(
        report.slowest_turn_ms >= report.median_turn_ms,
        "the slowest turn is not faster than the middle one"
    );
}

#[test]
fn a_delegates_work_is_filed_under_the_conversation_that_asked_for_it() {
    // The delegate's own spans name conversation `s2`. Asking about `s1` still
    // has to include them, because the panel's per-conversation view is about
    // the turn somebody ran and not about the session id a sub-agent happened
    // to be given.
    let traces = Traces::new();
    run_a_turn(&traces);

    let mine = taurus_host::traces::report(&traces, Some("s1"));
    assert_eq!(mine.turns, 1);
    assert_eq!(mine.recent[0].steps.len(), 5);

    // And the delegate's own id names nothing of its own. That is the same
    // rule seen from the other side: the work is filed under the turn that
    // asked for it, and `s2` is not a conversation anybody has open.
    let theirs = taurus_host::traces::report(&traces, Some("s2"));
    assert!(theirs.is_empty(), "a delegate's id is not a conversation");
}

#[test]
fn what_a_backend_reported_survives_the_whole_trip() {
    // The field names are the conventions' and they are spelled in three
    // places: the span macro, the recorder's visitor, and the report. A typo
    // in any of them is a number that is simply never there, and nothing else
    // fails.
    let traces = Traces::new();
    run_a_turn(&traces);
    let report = taurus_host::traces::report(&traces, None);

    let model = report
        .models
        .iter()
        .find(|m| m.name == "qwen3.6:27b")
        .expect("the model that answered");
    assert_eq!(model.provider, "ollama");
    assert_eq!(model.calls, 2);
    assert_eq!(model.input_tokens, 1_400, "1000 + 400");
    assert_eq!(model.output_tokens, 60, "40 + 20");
    assert_eq!(model.cached_tokens, Some(700), "500 + 200");

    let tools: Vec<&str> = report.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(tools.contains(&"read_file"), "{tools:?}");
    assert!(tools.contains(&"spawn"), "{tools:?}");
    let spawn = report.tools.iter().find(|t| t.name == "spawn").unwrap();
    assert!(spawn.nested, "a spawn holds the delegate's whole turn");
}
