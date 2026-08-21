//! Opens one of each span and exports it, to see what actually lands.
//!
//! Not a test — it needs a collector listening, which a test suite must not
//! require. It exists because everything about this crate is only true on the
//! wire: that the exporter connects, that the target filter lets the harness's
//! own spans through and nothing else, and that the field *names* survive the
//! trip as the conventions spell them. All three are invisible from inside the
//! process, and all three are the whole feature.
//!
//! ```text
//! OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
//!   cargo run -p taurus-telemetry --example emit
//! ```
use taurus_core::telemetry;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let _guard = telemetry_install();

    let turn = telemetry::turn_span("ollama", "qwen3.6:27b", "session-1");
    let entered = turn.enter();

    let chat = telemetry::chat_span("ollama", "qwen3.6:27b", "session-1");
    {
        let _entered = chat.enter();
        telemetry::record_usage(
            &chat,
            &taurus_provider::TokenUsage {
                input_tokens: 1204,
                output_tokens: 88,
                cache_read_input_tokens: Some(1024),
                ..Default::default()
            },
        );
        chat.record("gen_ai.response.model", "qwen3.6:27b");
        chat.record(
            "gen_ai.response.finish_reasons",
            telemetry::finish_reason(taurus_provider::StopReason::ToolUse),
        );
    }

    let tool = telemetry::tool_span("read_file", "tu_1");
    {
        let _entered = tool.enter();
        telemetry::record_error(&tool, "not_found");
    }

    drop(entered);
    println!("emitted a turn, a chat, and a tool span");
    // The guard flushes on drop. Without it a process this short exports
    // nothing at all, which is the failure this example exists to catch.
}

fn telemetry_install() -> taurus_telemetry::Guard {
    taurus_telemetry::install(EnvFilter::new("info"), "taurus-example", None)
}
