//! What installing this crate costs every *other* crate in the process.
//!
//! The spans a subscriber keeps are only half of what installing one decides.
//! The other half is what it makes every callsite in every dependency cost,
//! and that half is invisible: a layer with no filter declares interest in
//! everything, `tracing` takes the union across layers to set the global
//! maximum level, and the result is that `trace!` in reqwest, hyper, h2 and
//! rustls goes live — arguments formatted, spans allocated in the registry —
//! before being dropped because no layer actually wanted them.
//!
//! That is the path every streamed token from Anthropic, an OpenAI-compatible
//! backend, or Gemini travels. Nothing fails when it happens. Nothing is
//! logged. It is exactly the shape of regression that needs a test rather than
//! a reviewer, so this asserts the ceiling directly.
//!
//! `set_default` rather than `init`: a global subscriber can be installed once
//! per process, and the composition is what is being checked, not the
//! installing.

use taurus_core::telemetry::Traces;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// What the app asks for, and the quietest thing the CLI ever asks for.
const APP_FILTER: &str = "taurus_app=info,taurus_core=info";
const CLI_FILTER: &str = "warn";

/// A callsite from the HTTP stack every remote provider streams through.
macro_rules! http_stack_trace_enabled {
    () => {
        tracing::event_enabled!(target: "hyper::proto::h2", tracing::Level::TRACE)
    };
}

fn fmt(filter: &str) -> impl Layer<tracing_subscriber::Registry> {
    tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::new(filter))
}

#[test]
fn the_recorder_does_not_switch_on_every_callsite_in_the_process() {
    // The regression this file exists for. Unfiltered, the recorder took the
    // ceiling from INFO to TRACE and turned on the whole HTTP stack.
    let _guard = tracing_subscriber::registry()
        .with(fmt(APP_FILTER))
        .with(taurus_telemetry::sink::layer(Traces::new()))
        .set_default();

    assert_eq!(
        LevelFilter::current(),
        LevelFilter::INFO,
        "the recorder raised the global ceiling"
    );
    assert!(
        !http_stack_trace_enabled!(),
        "hyper's trace callsites are live, on the path every token travels"
    );
}

#[test]
fn a_quiet_cli_stays_quiet_apart_from_the_spans_the_panel_needs() {
    // `taurus run` asks for `warn` and nothing else, and the recorder still
    // has to see INFO spans to have anything to keep. INFO is therefore the
    // correct ceiling here, and TRACE would still be wrong.
    let _guard = tracing_subscriber::registry()
        .with(fmt(CLI_FILTER))
        .with(taurus_telemetry::sink::layer(Traces::new()))
        .set_default();

    assert_eq!(LevelFilter::current(), LevelFilter::INFO);
    assert!(!http_stack_trace_enabled!());
}

#[test]
fn the_spans_the_panel_needs_still_arrive_through_the_filter() {
    // The other half: a filter tight enough to fix the ceiling must not be so
    // tight that the ring stays empty. Asserting the ceiling alone would pass
    // for a layer that is switched off entirely.
    let traces = Traces::new();
    {
        let _guard = tracing_subscriber::registry()
            .with(fmt(APP_FILTER))
            .with(taurus_telemetry::sink::layer(traces.clone()))
            .set_default();

        let turn = taurus_core::telemetry::turn_span("anthropic", "claude", "s1");
        let _entered = turn.enter();
        let tool = taurus_core::telemetry::tool_span("read_file", "tu_1");
        drop(tool.enter());
    }

    let kept = traces.snapshot().records;
    assert_eq!(kept.len(), 2, "the turn and its tool call");
}
