//! Sends the spans somewhere, when somebody has said where.
//!
//! `taurus-core` emits the spans — [`taurus_core::telemetry`] holds the
//! vocabulary — and they go nowhere at all unless a collector is configured.
//! This crate is the part that changes that, and it is separate for one
//! reason: OpenTelemetry is a large dependency tree to make every build carry
//! for a feature most runs never turn on. Nothing in the harness proper links
//! against it.
//!
//! # Nothing leaves the machine unasked
//!
//! There is no default endpoint. Not localhost, not a vendor, not a
//! "telemetry is on unless you opt out" arrangement — a harness that reads
//! private repositories has no business having an opinion about where a
//! description of that work should be sent. An endpoint is a thing somebody
//! types.
//!
//! And what is sent is the *shape* of a turn: which model, how many tokens,
//! how long, which tools, what failed. Not the conversation. Carrying that
//! needs a second switch, deliberately awkward, and is off by default — see
//! [`taurus_core::telemetry::Capture`].
//!
//! # The one thing that is kept locally
//!
//! Everything above is about export, which is off until somebody configures
//! it. Alongside it this crate keeps the last few hundred finished spans in a
//! ring in memory — [`sink`] — because "why did that turn take ninety seconds"
//! is a question asked *at the moment it happens*, by somebody who has not set
//! up a collector and should not have to before they can see an answer. That
//! ring goes nowhere: no endpoint, no file, gone when the process is. It is
//! what the app's trace panel draws.
//!
//! # Why OTLP rather than a format of our own
//!
//! Because the interesting question — *why did that turn take ninety seconds*
//! — is answered by a flame graph, and every tool that draws one already
//! speaks this. Langfuse, Phoenix, Jaeger, Grafana Tempo, Honeycomb, and a
//! `docker run otel/opentelemetry-collector` on a laptop all read what this
//! sends, and the GenAI conventions mean they read the *fields* too rather
//! than showing a span called "chat" with an opaque bag beside it.

use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use taurus_core::telemetry::Traces;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

pub mod sink;

/// The environment variable OpenTelemetry itself defines.
///
/// Honored so that `OTEL_EXPORTER_OTLP_ENDPOINT=... taurus run ...` behaves the
/// way it does for every other instrumented program somebody has run this
/// month. It beats the configured setting, because an environment variable is
/// what a person reaches for when they want *this run* traced and nothing else.
pub const ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";

/// How long a shutdown waits for buffered spans to reach the collector.
///
/// Short. A collector that has gone away must not hold up quitting the app —
/// losing the last few spans of a session is a far better outcome than an
/// application that will not close.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

/// Keeps the exporter alive, and flushes it on the way out.
///
/// Held by `main` for the length of the process. Dropping it flushes what is
/// buffered, which matters most in the CLI: a `taurus run` that finished its
/// task and exited immediately would otherwise send nothing at all, and the
/// trace of a short turn is exactly the one somebody is watching for when they
/// first set this up.
pub struct Guard {
    provider: Option<SdkTracerProvider>,
    traces: Traces,
}

impl Guard {
    /// The spans this process has finished, for whoever draws them.
    ///
    /// A handle onto the same ring the recorder writes into, not a copy — see
    /// [`taurus_core::telemetry::store`]. Available whether or not an endpoint
    /// was configured, which is the point: the local read is what somebody has
    /// on the machine in front of them, and a collector is what they set up
    /// afterwards if they want history.
    pub fn traces(&self) -> Traces {
        self.traces.clone()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let Some(provider) = self.provider.take() else {
            return;
        };
        if let Err(e) = provider.shutdown_with_timeout(FLUSH_TIMEOUT) {
            // A warning and not an error: the work is done, the process is
            // ending, and a failure to report on it is not a failure to do it.
            tracing::warn!(error = %e, "could not flush traces on the way out");
        }
    }
}

/// Installs the log subscriber, exporting traces too when an endpoint is set.
///
/// One function rather than two because there is only one subscriber and it can
/// only be installed once. Splitting it into "set up logging" and "maybe add
/// tracing" invites a second `.init()` on a path somebody forgot about, which
/// fails at runtime, on startup, in a way no test covers.
///
/// `configured` is what settings said; the environment variable wins if it is
/// set. Neither present means no exporter is built at all — not one pointed at
/// nowhere — so the cost of leaving this alone is a branch taken once.
///
/// The *local* recorder is installed either way. It is a bounded ring in this
/// process that nothing sends anywhere — see [`sink`] — and it is what the
/// app's own trace panel reads. Making that conditional on an endpoint would
/// mean the first person to ask "why was that slow" is told to go and stand up
/// a collector and then do it again.
pub fn install(filter: EnvFilter, service: &str, configured: Option<&str>) -> Guard {
    let traces = Traces::new();
    let endpoint = std::env::var(ENDPOINT_ENV)
        .ok()
        .filter(|e| !e.trim().is_empty())
        .or_else(|| configured.map(str::to_string))
        .filter(|e| !e.trim().is_empty());

    let Some(endpoint) = endpoint else {
        tracing_subscriber::registry()
            .with(fmt_layer(filter))
            .with(sink::Recorder::new(traces.clone()))
            .init();
        return Guard {
            provider: None,
            traces,
        };
    };

    match provider(&endpoint, service) {
        Ok(provider) => {
            let tracer = provider.tracer("taurus");
            tracing_subscriber::registry()
                .with(fmt_layer(filter))
                .with(sink::Recorder::new(traces.clone()))
                // Only the harness's own spans are exported. Without this the
                // trace fills with reqwest, hyper, and rustls internals — every
                // one of them a real span and none of them the thing anybody
                // opened the trace to look at.
                .with(
                    tracing_opentelemetry::layer()
                        .with_tracer(tracer)
                        .with_filter(
                            tracing_subscriber::filter::Targets::new()
                                .with_target("taurus::gen_ai", tracing::Level::INFO),
                        ),
                )
                .init();
            tracing::info!(%endpoint, "exporting traces");
            Guard {
                provider: Some(provider),
                traces,
            }
        }
        Err(e) => {
            // Logging still works, the turn still runs, and the local panel
            // still fills. A misconfigured collector is not a reason to refuse
            // to start — but it is a reason to say so once, loudly, because the
            // alternative is somebody watching an empty dashboard and
            // concluding the harness is broken.
            tracing_subscriber::registry()
                .with(fmt_layer(filter))
                .with(sink::Recorder::new(traces.clone()))
                .init();
            tracing::error!(
                %endpoint,
                error = %e,
                "traces will not be exported; everything else is unaffected"
            );
            Guard {
                provider: None,
                traces,
            }
        }
    }
}

fn fmt_layer<S>(filter: EnvFilter) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_filter(filter)
}

fn provider(endpoint: &str, service: &str) -> Result<SdkTracerProvider, String> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_attributes([
                    KeyValue::new("service.name", service.to_string()),
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ])
                .build(),
        )
        .build())
}

#[cfg(test)]
mod tests {
    /// The endpoint resolution alone, which is the part with a rule in it.
    /// Installing a subscriber is process-global and can only happen once, so
    /// it is not something a test suite can exercise twice.
    fn resolve(env: Option<&str>, configured: Option<&str>) -> Option<String> {
        env.map(str::to_string)
            .filter(|e| !e.trim().is_empty())
            .or_else(|| configured.map(str::to_string))
            .filter(|e| !e.trim().is_empty())
    }

    #[test]
    fn nothing_configured_means_no_exporter_rather_than_one_pointed_at_localhost() {
        // The default has to be *off*, not "off-ish". A harness that reads
        // private repositories guessing a destination would be a bug with a
        // very long tail.
        assert_eq!(resolve(None, None), None);
        assert_eq!(resolve(Some("  "), Some("")), None);
    }

    #[test]
    fn the_standard_variable_beats_the_setting() {
        // An environment variable is what somebody reaches for to trace one
        // run. If the saved setting won, that would silently not work.
        assert_eq!(
            resolve(Some("http://localhost:4318"), Some("http://saved:4318")),
            Some("http://localhost:4318".to_string())
        );
    }

    #[test]
    fn a_blank_variable_falls_through_to_the_setting() {
        // `OTEL_EXPORTER_OTLP_ENDPOINT=` in a shell profile is unset, not a
        // request to disable the thing the settings file turned on.
        assert_eq!(
            resolve(Some(""), Some("http://saved:4318")),
            Some("http://saved:4318".to_string())
        );
    }
}
