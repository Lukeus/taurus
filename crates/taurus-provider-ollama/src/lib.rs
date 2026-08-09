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

use wire::{ChatBody, ChatChunk, Options, ShowResponse, TagsResponse};

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";
const PROVIDER_ID: &str = "ollama";

pub struct OllamaProvider {
    base_url: String,
    client: reqwest::Client,
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
            caps: Arc::new(Mutex::new(HashMap::new())),
        }
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
                let size = m.details.and_then(|d| d.parameter_size);
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
            context_length: show.context_length().unwrap_or(8192),
        };

        debug!(model, ?caps, "resolved ollama capabilities");
        self.caps.lock().await.insert(model.to_string(), caps);
        Ok(caps)
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
