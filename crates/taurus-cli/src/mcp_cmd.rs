//! `taurus mcp cost` — what one MCP server's tools actually cost, measured.
//!
//! The MCP panel and `taurus usage` price a schema at four characters a token.
//! That's an estimate of the text, and what a backend bills for is the request
//! as it frames it: its own tool-use preamble, its tokenizer, and whatever it
//! does with a schema before counting it. The only way to know that number is
//! to ask the backend, and this does: the same tiny request twice, once with
//! every tool this session has and once without one server's, and the
//! difference in the input tokens the backend reports is what that server
//! costs on every request of every turn.
//!
//! It spends real tokens, a few hundred input and a handful of output per
//! request, two requests per repetition. That is said before it starts.
//!
//! What it doesn't measure: whether the model does better or worse with the
//! server there, which takes real tasks and a way to grade them; and the
//! cost on a backend that doesn't report usage, which is said rather than
//! shown as zero.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Subcommand;
use taurus_host::Host;
use taurus_provider::{ChatRequest, Message, Provider, StreamEvent, ToolDef};
use tokio_util::sync::CancellationToken;

#[derive(Subcommand)]
pub enum McpCommand {
    /// Measure what one server's tools add to every request, by asking the
    /// provider. Sends 2 × REPETITIONS small requests on the session's model.
    Cost {
        /// The server, by its key in mcp.json.
        server: String,
        /// How many times to measure. More smooths out a backend whose counts
        /// wobble; the median is what's reported.
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=10))]
        repetitions: u32,
    },
}

/// How long one measuring request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// What a measuring request asks. Short, and answerable without a tool, so
/// the answer costs a few output tokens and no model calls one.
const ASK: &str = "Reply with the single word OK. Do not call any tool.";

pub async fn run(
    host: &Host,
    command: McpCommand,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<ExitCode, String> {
    match command {
        McpCommand::Cost {
            server,
            repetitions,
        } => cost(host, &server, repetitions, provider, model).await,
    }
}

async fn cost(
    host: &Host,
    server: &str,
    repetitions: u32,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<ExitCode, String> {
    let statuses = host.mcp_statuses().await;
    let Some(status) = statuses.iter().find(|s| s.name == server) else {
        let known: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        return Err(if known.is_empty() {
            "no MCP servers are configured".to_string()
        } else {
            format!(
                "no server called '{server}'. Configured: {}",
                known.join(", ")
            )
        });
    };
    if !status.connected {
        return Err(match &status.error {
            Some(error) => {
                format!("'{server}' isn't running, so there's nothing to measure: {error}")
            }
            None => format!("'{server}' is switched off. Switch it on to measure it."),
        });
    }

    let prefix = taurus_mcp::namespaced(server, "");
    let every = host.tool_definitions().await;
    let (theirs, rest): (Vec<ToolDef>, Vec<ToolDef>) = every
        .into_iter()
        .partition(|def| def.name.starts_with(&prefix));
    if theirs.is_empty() {
        println!(
            "'{server}' offers no tools, or all of them are switched off, so it costs nothing."
        );
        return Ok(ExitCode::SUCCESS);
    }
    let estimate: u32 = theirs.iter().map(taurus_host::usage::schema_cost).sum();
    let mut with = rest.clone();
    with.extend(theirs.iter().cloned());

    let (provider_id, model) = host.resolve_model(provider, model).await?;
    let backend = host.provider(&provider_id).await?;
    println!(
        "Measuring '{server}' ({} tools) on {provider_id}/{model}: {} requests, each a few \
         hundred input tokens and a few output.",
        theirs.len(),
        repetitions * 2
    );

    let measured = measure(backend, &model, rest, with, repetitions).await?;
    print!("{}", report(server, estimate, &measured));
    Ok(ExitCode::SUCCESS)
}

/// The input tokens of each pair: (without the server, with it).
#[derive(Debug, Default, PartialEq)]
pub struct Measured {
    pub pairs: Vec<(u32, u32)>,
    /// Pairs where the backend said nothing about usage.
    pub unreported: u32,
}

impl Measured {
    /// The median of the differences, when there is one.
    pub fn median(&self) -> Option<i64> {
        let mut deltas: Vec<i64> = self
            .pairs
            .iter()
            .map(|(without, with)| i64::from(*with) - i64::from(*without))
            .collect();
        if deltas.is_empty() {
            return None;
        }
        deltas.sort_unstable();
        let mid = deltas.len() / 2;
        Some(if deltas.len() % 2 == 1 {
            deltas[mid]
        } else {
            (deltas[mid - 1] + deltas[mid]) / 2
        })
    }
}

/// Sends the pairs. Alternate pairs go in the other order, so whatever a
/// backend does differently on the second of two near-identical requests —
/// a cache hit, a warmer connection — doesn't always land on the same side.
pub async fn measure(
    backend: Arc<dyn Provider>,
    model: &str,
    without: Vec<ToolDef>,
    with: Vec<ToolDef>,
    repetitions: u32,
) -> Result<Measured, String> {
    let mut measured = Measured::default();
    for round in 0..repetitions {
        let (first, second) = if round % 2 == 0 {
            (&without, &with)
        } else {
            (&with, &without)
        };
        let a = input_tokens(&backend, model, first.clone()).await?;
        let b = input_tokens(&backend, model, second.clone()).await?;
        let (lean, full) = if round % 2 == 0 { (a, b) } else { (b, a) };
        match (lean, full) {
            (Some(lean), Some(full)) => measured.pairs.push((lean, full)),
            _ => measured.unreported += 1,
        }
    }
    Ok(measured)
}

/// The input tokens the backend reports for one request with these tools, or
/// `None` when it reports none.
async fn input_tokens(
    backend: &Arc<dyn Provider>,
    model: &str,
    tools: Vec<ToolDef>,
) -> Result<Option<u32>, String> {
    let request = ChatRequest {
        max_tokens: Some(16),
        ..ChatRequest::new(model, vec![Message::user(ASK)])
            .with_system(ASK)
            .with_tools(tools)
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let cancel = CancellationToken::new();
    let streaming = {
        let backend = backend.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { backend.stream(request, tx, cancel).await })
    };
    let mut usage = None;
    let reading = async {
        while let Some(event) = rx.recv().await {
            // Kept from the first event that carries input tokens. A
            // backend may send usage more than once, the later ones about
            // output only, and a zero there doesn't unsay the count.
            if let StreamEvent::Usage { usage: reported } = event {
                if reported.input_tokens > 0 {
                    usage = Some(reported.input_tokens);
                }
            }
        }
    };
    if tokio::time::timeout(REQUEST_TIMEOUT, reading)
        .await
        .is_err()
    {
        cancel.cancel();
        return Err(format!(
            "the provider didn't finish a request within {}s",
            REQUEST_TIMEOUT.as_secs()
        ));
    }
    streaming
        .await
        .map_err(|e| format!("the request failed: {e}"))?
        .map_err(|e| format!("the provider refused the request: {e}"))?;
    Ok(usage)
}

/// What to print.
pub fn report(server: &str, estimate: u32, measured: &Measured) -> String {
    let mut out = String::new();
    let Some(median) = measured.median() else {
        out.push_str(&format!(
            "The provider didn't report token usage, so there's nothing to compare. The \
             estimate is ~{estimate} tokens per request.\n"
        ));
        return out;
    };
    let deltas: Vec<String> = measured
        .pairs
        .iter()
        .map(|(without, with)| format!("{:+}", i64::from(*with) - i64::from(*without)))
        .collect();
    out.push_str(&format!(
        "'{server}' adds {median} input tokens to every request (median of {}: {}).\n",
        measured.pairs.len(),
        deltas.join(", ")
    ));
    out.push_str(&format!(
        "Estimated at four characters a token, it's ~{estimate}."
    ));
    if estimate > 0 && median > 0 {
        let ratio = median as f64 / f64::from(estimate);
        out.push_str(&format!(" The provider counts {ratio:.2}× that.\n"));
    } else {
        out.push('\n');
    }
    if median <= 0 {
        out.push_str(
            "A difference at or below zero isn't a free server. It means this backend's count \
             doesn't move with the tool list, or moved for some other reason between the two \
             requests. Try more --repetitions.\n",
        );
    }
    if measured.unreported > 0 {
        out.push_str(&format!(
            "{} pair(s) came back without usage and were left out.\n",
            measured.unreported
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use taurus_provider::{Capabilities, ModelInfo, StopReason, TokenUsage};
    use tokio::sync::mpsc;

    /// Bills ten tokens per tool, plus a fixed hundred for the rest.
    struct Counting;

    #[async_trait]
    impl Provider for Counting {
        fn id(&self) -> &str {
            "counting"
        }
        async fn models(&self) -> taurus_provider::Result<Vec<ModelInfo>> {
            Ok(Vec::new())
        }
        async fn capabilities(&self, _: &str) -> taurus_provider::Result<Capabilities> {
            Ok(Capabilities::default())
        }
        async fn stream(
            &self,
            request: ChatRequest,
            tx: mpsc::Sender<StreamEvent>,
            _cancel: CancellationToken,
        ) -> taurus_provider::Result<StopReason> {
            let usage = TokenUsage {
                input_tokens: 100 + 10 * request.tools.len() as u32,
                output_tokens: 1,
                ..TokenUsage::default()
            };
            let _ = tx.send(StreamEvent::Usage { usage }).await;
            Ok(StopReason::EndTurn)
        }
    }

    fn tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: String::new(),
            input_schema: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn the_difference_is_what_the_server_adds() {
        let without = vec![tool("read_file")];
        let with = vec![tool("read_file"), tool("mcp__s__a"), tool("mcp__s__b")];
        let measured = measure(Arc::new(Counting), "m", without, with, 3)
            .await
            .unwrap();
        assert_eq!(measured.pairs, vec![(110, 130); 3]);
        assert_eq!(measured.median(), Some(20));
        let text = report("s", 15, &measured);
        assert!(text.contains("'s' adds 20 input tokens"), "{text}");
        assert!(text.contains("1.33×"), "{text}");
    }

    /// Reports input up front, then a closing event about output only.
    struct TwoEvents;

    #[async_trait]
    impl Provider for TwoEvents {
        fn id(&self) -> &str {
            "two-events"
        }
        async fn models(&self) -> taurus_provider::Result<Vec<ModelInfo>> {
            Ok(Vec::new())
        }
        async fn capabilities(&self, _: &str) -> taurus_provider::Result<Capabilities> {
            Ok(Capabilities::default())
        }
        async fn stream(
            &self,
            request: ChatRequest,
            tx: mpsc::Sender<StreamEvent>,
            _cancel: CancellationToken,
        ) -> taurus_provider::Result<StopReason> {
            let first = TokenUsage {
                input_tokens: 50 + request.tools.len() as u32,
                ..TokenUsage::default()
            };
            let last = TokenUsage {
                output_tokens: 2,
                ..TokenUsage::default()
            };
            let _ = tx.send(StreamEvent::Usage { usage: first }).await;
            let _ = tx.send(StreamEvent::Usage { usage: last }).await;
            Ok(StopReason::EndTurn)
        }
    }

    #[tokio::test]
    async fn a_closing_usage_event_without_input_does_not_erase_the_count() {
        let measured = measure(
            Arc::new(TwoEvents),
            "m",
            vec![tool("a")],
            vec![tool("a"), tool("b")],
            1,
        )
        .await
        .unwrap();
        assert_eq!(measured.pairs, vec![(51, 52)]);
        assert_eq!(measured.unreported, 0);
    }

    #[test]
    fn the_median_of_an_even_count_is_the_middle_two_averaged() {
        let measured = Measured {
            pairs: vec![(0, 10), (0, 20), (0, 30), (0, 100)],
            unreported: 0,
        };
        assert_eq!(measured.median(), Some(25));
    }

    #[test]
    fn no_usage_is_said_rather_than_shown_as_zero() {
        let text = report("s", 42, &Measured::default());
        assert!(text.contains("didn't report token usage"), "{text}");
        assert!(text.contains("~42"), "{text}");
    }
}
