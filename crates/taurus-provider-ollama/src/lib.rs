//! Ollama adapter.
//!
//! Handles the two things that make local models awkward: capabilities vary
//! per model on the same server (`qwen3.6` has native tools, `gemma3` does
//! not), and the transport is newline-delimited JSON rather than SSE. Callers
//! see neither — just [`Provider`].

mod convert;
mod wire;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use taurus_provider::prompted::{PromptedScanner, PromptedTools};
use taurus_provider::{
    Capabilities, ChatRequest, ModelInfo, Provider, ProviderError, Result, StopReason, StreamEvent,
    TokenUsage,
};

use wire::{ChatBody, ChatChunk, EmbedResponse, Options, ShowResponse, TagsResponse};

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";
const PROVIDER_ID: &str = "ollama";

/// The largest window this asks Ollama to allocate, unless configured otherwise.
///
/// A model reports the window it was *trained* for, and Ollama allocates that
/// much when nothing says otherwise. Those are not the same question. On a
/// machine that has to hold the weights and the KV cache at once, the trained
/// window is often far past the point where the model still runs well, and the
/// symptom is not an error — it is a model that answers so slowly it reads as
/// broken.
///
/// Measured on `qwen3-coder:30b` (trained window 262,144) with an ordinary
/// 9,019-token agent prompt, warm, on one machine:
///
/// | allocated | prompt eval | total  | VRAM    |
/// |-----------|-------------|--------|---------|
/// | 262,144   | 202.8s      | 233.3s | 29.0 GB |
/// | 32,768    | 10.7s       | 10.8s  | 21.7 GB |
///
/// A turn is a dozen of those. The difference is between a local model that
/// works and one nobody waits for.
///
/// Not sized per request, deliberately: changing this value makes Ollama
/// reallocate the cache and reload the model — measured at five seconds a time
/// against six milliseconds for a request that leaves it alone. It has to be
/// one stable number per model, which is what makes it configuration rather
/// than something computed from the prompt in hand.
///
/// 32,768 because the harness compacts at a fraction of the window, so this is
/// a working history of roughly 26,000 tokens — more than a coding turn spends
/// — while staying inside what a machine that can run the weights can also
/// hold. A model trained for less than this keeps its own smaller number; a
/// machine with room for more says so with `context_length` in `providers.json`.
pub const DEFAULT_CONTEXT_LIMIT: u32 = 32_768;

/// What a model reports when `/api/show` will not answer at all.
const UNKNOWN_CONTEXT: u32 = 8192;

pub struct OllamaProvider {
    base_url: String,
    client: reqwest::Client,
    /// Ceiling on the window, whatever the model says it was trained for.
    context_limit: u32,
    /// `/api/show` costs a round trip and the answer never changes for a given
    /// model tag, so resolve it once per process.
    caps: Arc<Mutex<HashMap<String, Capabilities>>>,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url,
            client: reqwest::Client::new(),
            context_limit: DEFAULT_CONTEXT_LIMIT,
            caps: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Raises or lowers the ceiling from [`DEFAULT_CONTEXT_LIMIT`].
    ///
    /// `None` keeps the default. The value is a ceiling rather than a setting:
    /// a model trained for less than this is still served its own smaller
    /// window, because asking for more than a model has is not a window, it is
    /// an error.
    pub fn with_context_limit(mut self, limit: Option<u32>) -> Self {
        if let Some(limit) = limit.filter(|l| *l > 0) {
            self.context_limit = limit;
        }
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn unreachable(&self, source: reqwest::Error) -> ProviderError {
        ProviderError::Unreachable {
            provider: PROVIDER_ID.into(),
            base_url: self.base_url.clone(),
            source: Box::new(source),
        }
    }

    async fn post_json<B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<reqwest::Response> {
        let response = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        self.check_status(response).await
    }

    async fn check_status(&self, response: reqwest::Response) -> Result<reqwest::Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        // Ollama reports an unpulled model as a 404 with a "not found" body;
        // surfacing that as a plain HTTP error would send the user hunting for
        // a network problem they do not have.
        if status.as_u16() == 404 && body.contains("not found") {
            return Err(ProviderError::ModelNotFound {
                provider: PROVIDER_ID.into(),
                model: body.trim().to_string(),
            });
        }
        Err(ProviderError::Api {
            provider: PROVIDER_ID.into(),
            status: status.as_u16(),
            body,
        })
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn id(&self) -> &str {
        PROVIDER_ID
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        let response = self
            .client
            .get(self.url("/api/tags"))
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        let response = self.check_status(response).await?;
        let tags: TagsResponse = response.json().await.map_err(|e| self.unreachable(e))?;
        Ok(tags
            .models
            .into_iter()
            .map(|m| {
                // Ollama reports an empty parameter_size for some builds; an
                // empty "()" suffix reads as a bug to the user.
                let size = m
                    .details
                    .and_then(|d| d.parameter_size)
                    .filter(|s| !s.trim().is_empty());
                let display_name = match size {
                    Some(s) => format!("{} ({s})", m.name),
                    None => m.name.clone(),
                };
                ModelInfo {
                    id: m.name,
                    display_name,
                    context_length: None,
                }
            })
            .collect())
    }

    async fn capabilities(&self, model: &str) -> Result<Capabilities> {
        if let Some(cached) = self.caps.lock().await.get(model) {
            return Ok(*cached);
        }

        let response = self
            .post_json("/api/show", &serde_json::json!({ "model": model }))
            .await?;
        let show: ShowResponse = response.json().await.map_err(|e| self.unreachable(e))?;

        let has = |c: &str| show.capabilities.iter().any(|x| x == c);
        let caps = Capabilities {
            native_tools: has("tools"),
            vision: has("vision"),
            thinking: has("thinking"),
            // Capped rather than reported. This one number is both what
            // compaction plans against and what the request asks Ollama to
            // allocate, so the two cannot come to disagree — and a harness that
            // planned for a window the server was not serving would fill a
            // prompt the server then silently truncated from the front, taking
            // the system prompt and the tools with it.
            context_length: show
                .context_length()
                .unwrap_or(UNKNOWN_CONTEXT)
                .min(self.context_limit),
        };

        debug!(model, ?caps, "resolved ollama capabilities");
        self.caps.lock().await.insert(model.to_string(), caps);
        Ok(caps)
    }

    /// `/api/embed`, which takes a batch and answers in the order it was given.
    ///
    /// Batched by the caller rather than here: the index knows how many chunks
    /// it is willing to hold in memory at once and this does not, and a request
    /// split behind the caller's back would make the progress it reports a
    /// fiction.
    ///
    /// Two failures are worth recognising by hand, because both are ordinary
    /// setup states that arrive looking like faults. An embedding model that
    /// was never pulled comes back the same way any missing model does, which
    /// `check_status` already turns into `ModelNotFound` — the message just has
    /// to name the pull. A server built without embedding support answers 400
    /// with prose, and passing that through as a raw API error would send
    /// someone looking for a bad request they did not make.
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .post_json(
                "/api/embed",
                &serde_json::json!({ "model": model, "input": inputs }),
            )
            .await
            .map_err(|e| match e {
                ProviderError::ModelNotFound { provider, model } => ProviderError::Protocol(
                    format!("'{model}' is not pulled on {provider}. Run `ollama pull {model}`."),
                ),
                ProviderError::Api { body, .. } if body.contains("does not support embeddings") => {
                    ProviderError::Protocol(
                        "this Ollama server was started without embedding support. Restart it \
                         with `--embeddings`, or point the index at one that has them."
                            .into(),
                    )
                }
                other => other,
            })?;

        let embedded: EmbedResponse = response.json().await.map_err(|e| self.unreachable(e))?;

        // A backend that answered with a different number of vectors than it
        // was given texts has broken the only contract that makes the result
        // usable: position is the only thing tying a vector to its chunk.
        if embedded.embeddings.len() != inputs.len() {
            return Err(ProviderError::Protocol(format!(
                "asked {} for {} embeddings and got {}",
                PROVIDER_ID,
                inputs.len(),
                embedded.embeddings.len()
            )));
        }
        Ok(embedded.embeddings)
    }

    async fn stream(
        &self,
        mut request: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> Result<StopReason> {
        let caps = self.capabilities(&request.model).await?;

        // The decision that keeps tool-less models usable. Everything after
        // this point is identical for both paths except how text is parsed.
        let prompted = !caps.native_tools && !request.tools.is_empty();
        if prompted {
            warn!(
                model = %request.model,
                "model has no native tool support; using prompted tool calling"
            );
            PromptedTools::rewrite(&mut request);
        }

        let options = Options {
            temperature: request.temperature,
            num_predict: request.max_tokens,
            // The same number compaction is planning against, by construction.
            num_ctx: Some(caps.context_length),
            stop: request.stop_sequences.clone(),
        };
        let body = ChatBody {
            model: request.model.clone(),
            messages: convert::messages_to_wire(&request),
            stream: true,
            tools: convert::tools_to_wire(&request.tools),
            think: caps.thinking.then_some(true),
            options: (!options.is_empty()).then_some(options),
        };

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(StopReason::Canceled),
            r = self.post_json("/api/chat", &body) => r?,
        };

        let mut reader = NdjsonReader::new(response.bytes_stream());
        let mut scanner = prompted.then(PromptedScanner::new);
        let mut saw_tool_call = false;
        let mut usage = TokenUsage::default();
        let mut done_reason = None;

        loop {
            let line = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(StopReason::Canceled),
                next = reader.next_line() => next,
            };
            let Some(line) = line.map_err(|e| self.unreachable(e))? else {
                break;
            };

            let chunk: ChatChunk = match serde_json::from_str(&line) {
                Ok(c) => c,
                // A single unparseable line is not worth failing the turn over;
                // the stream is otherwise well formed and self-terminating.
                Err(e) => {
                    warn!(error = %e, line = %line, "skipping malformed ollama chunk");
                    continue;
                }
            };

            if let Some(error) = chunk.error {
                return Err(ProviderError::Api {
                    provider: PROVIDER_ID.into(),
                    status: 200,
                    body: error,
                });
            }

            if let Some(message) = chunk.message {
                if let Some(thinking) = message.thinking.filter(|t| !t.is_empty()) {
                    send(&tx, StreamEvent::ThinkingDelta { text: thinking }).await?;
                }

                if !message.content.is_empty() {
                    match scanner.as_mut() {
                        Some(scanner) => {
                            for event in scanner.feed(&message.content) {
                                send(&tx, event).await?;
                            }
                        }
                        None => {
                            send(
                                &tx,
                                StreamEvent::TextDelta {
                                    text: message.content,
                                },
                            )
                            .await?;
                        }
                    }
                }

                for call in message.tool_calls {
                    saw_tool_call = true;
                    let id = call.id.unwrap_or_else(taurus_provider::new_tool_use_id);
                    send(
                        &tx,
                        StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: call.function.name,
                        },
                    )
                    .await?;
                    send(
                        &tx,
                        StreamEvent::ToolUseInputDelta {
                            id: id.clone(),
                            json: call.function.arguments.to_string(),
                        },
                    )
                    .await?;
                    send(&tx, StreamEvent::ToolUseEnd { id }).await?;
                }
            }

            if chunk.done {
                usage = TokenUsage {
                    input_tokens: chunk.prompt_eval_count.unwrap_or(0),
                    output_tokens: chunk.eval_count.unwrap_or(0),
                };
                done_reason = chunk.done_reason;
                break;
            }
        }

        if let Some(scanner) = scanner.as_mut() {
            for event in scanner.finish() {
                send(&tx, event).await?;
            }
            saw_tool_call |= scanner.saw_tool_call();
        }

        send(&tx, StreamEvent::Usage { usage }).await?;

        Ok(if saw_tool_call {
            StopReason::ToolUse
        } else {
            match done_reason.as_deref() {
                Some("length") => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            }
        })
    }
}

/// A dropped receiver means the session went away; treat it as cancellation
/// rather than an error so the caller does not report a failure to the user.
async fn send(tx: &mpsc::Sender<StreamEvent>, event: StreamEvent) -> Result<()> {
    tx.send(event).await.map_err(|_| ProviderError::Canceled)
}

/// Splits a byte stream into newline-delimited JSON records.
struct NdjsonReader<S> {
    stream: S,
    buf: Vec<u8>,
    done: bool,
}

impl<S> NdjsonReader<S>
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
{
    fn new(stream: S) -> Self {
        Self {
            stream,
            buf: Vec::new(),
            done: false,
        }
    }

    async fn next_line(&mut self) -> reqwest::Result<Option<String>> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line = self.buf.drain(..=i).collect::<Vec<_>>();
                let line = String::from_utf8_lossy(&line[..line.len() - 1])
                    .trim()
                    .to_string();
                if line.is_empty() {
                    continue;
                }
                return Ok(Some(line));
            }
            if self.done {
                // Final record without a trailing newline.
                let rest = String::from_utf8_lossy(&std::mem::take(&mut self.buf))
                    .trim()
                    .to_string();
                return Ok((!rest.is_empty()).then_some(rest));
            }
            match self.stream.next().await {
                Some(Ok(bytes)) => self.buf.extend_from_slice(&bytes),
                Some(Err(e)) => return Err(e),
                None => self.done = true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_provider::Message;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A server that answers `/api/show` for a model with the given trained
    /// window, and `/api/chat` with an empty final chunk.
    async fn server_reporting(trained: u32) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "capabilities": ["completion", "tools"],
                "model_info": { "qwen3moe.context_length": trained },
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "{\"message\":{\"role\":\"assistant\",\"content\":\"ok\"},\"done\":true,\"done_reason\":\"stop\"}\n",
                "application/x-ndjson",
            ))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_window_past_what_a_machine_can_serve_is_capped() {
        // The reported number is what the model was trained for, not what the
        // machine in front of it can hold.
        let server = server_reporting(262_144).await;
        let provider = OllamaProvider::new(server.uri());
        let caps = provider.capabilities("qwen3-coder").await.unwrap();
        assert_eq!(caps.context_length, DEFAULT_CONTEXT_LIMIT);
    }

    #[tokio::test]
    async fn a_model_trained_for_less_keeps_its_own_window() {
        // A ceiling, not a setting. Asking for more window than a model has is
        // not a larger window, it is an error.
        let server = server_reporting(8_192).await;
        let provider = OllamaProvider::new(server.uri());
        let caps = provider.capabilities("small").await.unwrap();
        assert_eq!(caps.context_length, 8_192);
    }

    #[tokio::test]
    async fn a_configured_limit_replaces_the_default() {
        let server = server_reporting(262_144).await;
        let provider = OllamaProvider::new(server.uri()).with_context_limit(Some(131_072));
        let caps = provider.capabilities("qwen3-coder").await.unwrap();
        assert_eq!(caps.context_length, 131_072);

        // And is still a ceiling rather than a demand.
        let server = server_reporting(4_096).await;
        let provider = OllamaProvider::new(server.uri()).with_context_limit(Some(131_072));
        let caps = provider.capabilities("small").await.unwrap();
        assert_eq!(caps.context_length, 4_096);
    }

    #[tokio::test]
    async fn an_unset_limit_keeps_the_default() {
        let server = server_reporting(262_144).await;
        let provider = OllamaProvider::new(server.uri()).with_context_limit(None);
        let caps = provider.capabilities("qwen3-coder").await.unwrap();
        assert_eq!(caps.context_length, DEFAULT_CONTEXT_LIMIT);
    }

    #[tokio::test]
    async fn the_request_allocates_exactly_the_window_compaction_plans_against() {
        // The failure this rules out is the two disagreeing: a harness filling
        // a prompt for a window the server was never asked to allocate, and a
        // server quietly dropping the front of it.
        let server = server_reporting(262_144).await;
        let provider = OllamaProvider::new(server.uri());
        let planned = provider.capabilities("qwen3-coder").await.unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        provider
            .stream(
                ChatRequest::new("qwen3-coder", vec![Message::user("go")]),
                tx,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let chat = requests
            .iter()
            .find(|r| r.url.path() == "/api/chat")
            .expect("a chat request");
        let body: serde_json::Value = serde_json::from_slice(&chat.body).unwrap();
        assert_eq!(body["options"]["num_ctx"], planned.context_length);
    }
}
