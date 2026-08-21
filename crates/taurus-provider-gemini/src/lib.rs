//! Google Gemini adapter.
//!
//! The fourth wire format here, and the one least like the normalized types.
//! Ollama and OpenAI differ from Anthropic in transport and in how a tool call
//! is spelled; Gemini differs in what a conversation *is*. The assistant is
//! called `model`, tool calls carry no ids, results are matched to calls by
//! name, schemas are an OpenAPI subset rather than JSON Schema, and every
//! streamed chunk is a whole response object rather than a delta envelope.
//!
//! None of that reached `taurus-core`, which is the point of the exercise: the
//! same claim the OpenAI adapter makes, tested against a format that shares
//! less with the normalized one.

mod convert;
mod wire;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use taurus_provider::{
    Capabilities, ChatRequest, ModelInfo, Provider, ProviderError, Result, StopReason, StreamEvent,
    TokenUsage,
};

use wire::{GenerateBody, ModelsResponse, StreamChunk};

pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// The API version segment. `v1beta` rather than `v1` because function calling,
/// system instructions, and thinking all live there.
const API_VERSION: &str = "v1beta";

/// What the listing endpoint cannot say.
///
/// It reports a context window per model but nothing about vision or tools, so
/// those two are configuration rather than probe results.
#[derive(Clone, Copy, Debug)]
pub struct GeminiCapabilities {
    pub vision: bool,
    pub context_length: u32,
}

impl Default for GeminiCapabilities {
    fn default() -> Self {
        Self {
            // Every Gemini model that serves `generateContent` takes images.
            vision: true,
            // Only reached when the listing is unavailable. Low on purpose:
            // guessing high compacts too late and surfaces as a provider error
            // mid-turn, guessing low costs some unnecessary compaction.
            context_length: 32_768,
        }
    }
}

pub struct GeminiProvider {
    id: String,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
    capabilities: GeminiCapabilities,
    models: Vec<String>,
    /// One listing per model, kept for the life of this provider.
    ///
    /// `capabilities` is asked once per iteration of the agent loop, because
    /// that is where compaction reads the context window — and answering it
    /// here means listing every model the account can see. Uncached, a ten-step
    /// turn would spend ten full listings re-learning one number that cannot
    /// change while the turn runs.
    probed: Arc<RwLock<HashMap<String, Capabilities>>>,
}

impl GeminiProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            client: reqwest::Client::new(),
            capabilities: GeminiCapabilities::default(),
            models: Vec::new(),
            probed: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }

    pub fn with_fallback_capabilities(mut self, capabilities: GeminiCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{API_VERSION}{path}", self.base_url)
    }

    /// Sends the key in a header rather than the query string.
    ///
    /// This API accepts `?key=`, and that is how most of its documentation
    /// spells it. A credential in a URL is a credential in every proxy log,
    /// every error report, and every retained request trace between here and
    /// Google, so it goes in a header that can also be marked sensitive.
    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let Some(key) = &self.api_key else {
            return builder;
        };
        let mut value = match reqwest::header::HeaderValue::from_str(key) {
            Ok(value) => value,
            Err(_) => {
                warn!(
                    provider = %self.id,
                    "the API key has characters an HTTP header cannot carry; sending none"
                );
                return builder;
            }
        };
        value.set_sensitive(true);
        builder.header("x-goog-api-key", value)
    }

    fn unreachable(&self, source: reqwest::Error) -> ProviderError {
        ProviderError::Unreachable {
            provider: self.id.clone(),
            base_url: self.base_url.clone(),
            source: Box::new(source),
        }
    }

    async fn check_status(&self, response: reqwest::Response) -> Result<reqwest::Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(ProviderError::MissingCredentials {
                provider: self.id.clone(),
            });
        }
        Err(ProviderError::Api {
            provider: self.id.clone(),
            status: status.as_u16(),
            body,
        })
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        if !self.models.is_empty() {
            return Ok(self
                .models
                .iter()
                .map(|id| ModelInfo {
                    id: id.clone(),
                    display_name: id.clone(),
                    context_length: None,
                })
                .collect());
        }

        let response = self
            .authorize(self.client.get(self.url("/models")))
            .query(&[("pageSize", "200")])
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        let response = self.check_status(response).await?;
        let models: ModelsResponse = response.json().await.map_err(|e| self.unreachable(e))?;

        Ok(models
            .models
            .into_iter()
            // An embedding model in the picker is one that fails at the first
            // turn rather than at the moment it was chosen.
            .filter(|m| m.generates())
            .map(|m| ModelInfo {
                display_name: m.display_name.clone().unwrap_or_else(|| m.id().to_string()),
                context_length: m.input_token_limit,
                id: m.id().to_string(),
            })
            .collect())
    }

    async fn capabilities(&self, model: &str) -> Result<Capabilities> {
        if let Some(cached) = self.probed.read().await.get(model) {
            return Ok(*cached);
        }

        // The listing carries the window, so it is asked for rather than
        // configured — but only that. Nothing there reports tool or image
        // support, so those two stay configuration.
        let context_length = self
            .models()
            .await
            .ok()
            .and_then(|models| {
                models
                    .into_iter()
                    .find(|m| m.id == model)
                    .and_then(|m| m.context_length)
            })
            .unwrap_or(self.capabilities.context_length);

        let capabilities = Capabilities {
            native_tools: true,
            vision: self.capabilities.vision,
            thinking: true,
            context_length,
        };

        // Cached even when the listing failed and this is the fallback: a
        // backend that would not answer once will not answer ten times in the
        // same turn, and retrying is a stall per iteration.
        self.probed
            .write()
            .await
            .insert(model.to_string(), capabilities);
        Ok(capabilities)
    }

    async fn stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> Result<StopReason> {
        let body = GenerateBody::from_request(&request);
        // The model is in the path, and the method is a suffix on it rather
        // than a separate route.
        let url = self.url(&format!("/models/{}:streamGenerateContent", request.model));

        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(StopReason::Canceled),
            r = self
                .authorize(self.client.post(&url).json(&body))
                // Without this the response is a JSON array delivered in
                // fragments rather than an event stream, and nothing below
                // would parse.
                .query(&[("alt", "sse")])
                .send() => {
                self.check_status(r.map_err(|e| self.unreachable(e))?).await?
            }
        };

        let mut reader = SseReader::new(response.bytes_stream());
        let mut usage = TokenUsage::default();
        let mut finish_reason = None;
        let mut saw_tool_call = false;
        // Ids are this harness's own: the wire format has none, and the
        // normalized form needs one per call to pair a result with it.
        let mut call_index = 0usize;

        loop {
            let data = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(StopReason::Canceled),
                next = reader.next_event() => next,
            };
            let Some(data) = data.map_err(|e| self.unreachable(e))? else {
                break;
            };

            let chunk: StreamChunk = match serde_json::from_str(&data) {
                Ok(chunk) => chunk,
                Err(e) => {
                    warn!(error = %e, "skipping malformed SSE chunk");
                    continue;
                }
            };

            // An error can arrive inside the stream after a 200, the same way
            // it can on the Anthropic adapter.
            if let Some(error) = chunk.error {
                return Err(ProviderError::Api {
                    provider: self.id.clone(),
                    status: error.code.unwrap_or(400),
                    body: format!("{}: {}", error.status, error.message),
                });
            }

            if let Some(u) = chunk.usage_metadata {
                usage = TokenUsage {
                    input_tokens: u.prompt_token_count.unwrap_or(usage.input_tokens),
                    output_tokens: u.output_total(),
                    cache_read_input_tokens: u.cached_content_token_count,
                    cache_creation_input_tokens: None,
                    // Already inside `output_total`, and reported separately so
                    // a turn that spent its budget thinking can be seen to have
                    // done so.
                    reasoning_tokens: u.thoughts_token_count,
                };
            }

            let Some(candidate) = chunk.candidates.into_iter().next() else {
                continue;
            };
            if let Some(reason) = candidate.finish_reason {
                finish_reason = Some(reason);
            }
            let Some(content) = candidate.content else {
                continue;
            };

            for part in content.parts {
                if let Some(call) = part.function_call {
                    saw_tool_call = true;
                    let id = call.id.unwrap_or_else(|| {
                        call_index += 1;
                        format!("call_{call_index}")
                    });
                    // Arrives whole rather than in fragments, so the three
                    // events go out back to back. Consumers cannot tell.
                    send(
                        &tx,
                        StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: call.name,
                        },
                    )
                    .await?;
                    send(
                        &tx,
                        StreamEvent::ToolUseInputDelta {
                            id: id.clone(),
                            json: call.args.to_string(),
                        },
                    )
                    .await?;
                    // Before the close, because it attaches to the block that
                    // is still open. The signature arrives on the part holding
                    // the call, not inside the call, and this API wants it back
                    // on that same part next request.
                    if let Some(signature) = part.thought_signature {
                        send(&tx, StreamEvent::ThinkingSignature { signature }).await?;
                    }
                    send(&tx, StreamEvent::ToolUseEnd { id }).await?;
                    continue;
                }

                let Some(text) = part.text else { continue };
                if part.thought.unwrap_or(false) {
                    send(&tx, StreamEvent::ThinkingDelta { text }).await?;
                    if let Some(signature) = part.thought_signature {
                        send(&tx, StreamEvent::ThinkingSignature { signature }).await?;
                    }
                } else if !text.is_empty() {
                    send(&tx, StreamEvent::TextDelta { text }).await?;
                }
            }
        }

        send(&tx, StreamEvent::Usage { usage }).await?;

        Ok(if saw_tool_call {
            StopReason::ToolUse
        } else {
            match finish_reason.as_deref() {
                Some("MAX_TOKENS") => StopReason::MaxTokens,
                Some("STOP") => StopReason::EndTurn,
                // SAFETY, RECITATION, and the rest end the turn with nothing
                // more to say, which is what EndTurn means here.
                _ => StopReason::EndTurn,
            }
        })
    }
}

async fn send(tx: &mpsc::Sender<StreamEvent>, event: StreamEvent) -> Result<()> {
    tx.send(event).await.map_err(|_| ProviderError::Canceled)
}

/// Minimal SSE reader: yields the payload of each `data:` line.
struct SseReader<S> {
    stream: S,
    buf: Vec<u8>,
    done: bool,
}

impl<S> SseReader<S>
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

    async fn next_event(&mut self) -> reqwest::Result<Option<String>> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1])
                    .trim()
                    .to_string();
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                if let Some(data) = line.strip_prefix("data:") {
                    return Ok(Some(data.trim().to_string()));
                }
                continue;
            }
            if self.done {
                return Ok(None);
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
    use taurus_provider::{ContentBlock, Message, StreamAccumulator};

    /// Serves recorded chunks as an event stream, drives the adapter through
    /// its own HTTP path, and folds the result with the shared accumulator, so
    /// the assertion is about the `Message` a turn produces.
    ///
    /// Through `stream()` rather than around it. This used to re-implement the
    /// adapter's part handling, which meant every test in here asserted against
    /// a copy — and the copy went on passing after the adapter learned to keep
    /// a signature the copy dropped.
    async fn replay(chunks: &[&str]) -> (Message, TokenUsage) {
        // Recorded chunks are written across lines for reading. A newline ends
        // a `data:` line, so each one is re-serialized onto one.
        let body: String = chunks
            .iter()
            .map(|raw| {
                let chunk: serde_json::Value = serde_json::from_str(raw).expect(raw);
                format!("data: {chunk}\n\n")
            })
            .collect();

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(body, "text/event-stream")
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let provider = GeminiProvider::new("gemini", server.uri(), None);
        let (tx, mut rx) = mpsc::channel(64);
        let turn = tokio::spawn(async move {
            provider
                .stream(
                    ChatRequest::new("gemini-2.5-pro", vec![Message::user("go")]),
                    tx,
                    CancellationToken::new(),
                )
                .await
        });

        let mut acc = StreamAccumulator::new();
        while let Some(event) = rx.recv().await {
            acc.push(event);
        }
        turn.await.expect("the turn task").expect("the turn");
        let (message, usage, _) = acc.finish();
        (message, usage)
    }

    #[tokio::test]
    async fn text_chunks_reassemble_into_one_block() {
        let (message, usage) = replay(&[
            r#"{"candidates":[{"content":{"parts":[{"text":"Hel"}],"role":"model"}}]}"#,
            r#"{"candidates":[{"content":{"parts":[{"text":"lo"}],"role":"model"},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":3}}"#,
        ])
        .await;
        assert_eq!(message.text(), "Hello");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 3);
    }

    #[tokio::test]
    async fn a_function_call_arrives_whole_and_still_looks_fragmented() {
        // The wire format hands over the whole call at once. Consumers must not
        // be able to tell it apart from a backend that streams the input.
        let (message, _) = replay(&[
            r#"{"candidates":[{"content":{"parts":[
                {"functionCall":{"name":"read_file","args":{"path":"a.txt"}}}],"role":"model"}}]}"#,
        ])
        .await;
        let (id, name, input) = message.tool_uses().next().expect("a tool call");
        assert_eq!(name, "read_file");
        assert_eq!(input, &serde_json::json!({"path": "a.txt"}));
        // Synthesized, because the wire format carries none.
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn two_calls_to_the_same_tool_get_different_ids() {
        // The reason ids are synthesized at all. With names alone these two are
        // indistinguishable, and their results cannot be told apart either.
        let (message, _) = replay(&[
            r#"{"candidates":[{"content":{"parts":[
                {"functionCall":{"name":"read_file","args":{"path":"a.txt"}}},
                {"functionCall":{"name":"read_file","args":{"path":"b.txt"}}}],"role":"model"}}]}"#,
        ])
        .await;
        let ids: Vec<&str> = message.tool_uses().map(|(id, _, _)| id).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    #[tokio::test]
    async fn a_provider_supplied_id_is_preferred_over_a_synthesized_one() {
        let (message, _) = replay(&[r#"{"candidates":[{"content":{"parts":[
                {"functionCall":{"id":"fc_real","name":"x","args":{}}}],"role":"model"}}]}"#])
        .await;
        assert_eq!(message.tool_uses().next().unwrap().0, "fc_real");
    }

    #[tokio::test]
    async fn a_signed_call_keeps_its_signature() {
        // The signature rides the part holding the call, not the call. This
        // API refuses a request whose history replays the call without it.
        let (message, _) = replay(&[r#"{"candidates":[{"content":{"parts":[
                {"functionCall":{"name":"run_command","args":{"cmd":"ls"}},
                 "thoughtSignature":"sig-call"}],"role":"model"}}]}"#])
        .await;
        let ContentBlock::ToolUse { signature, .. } = &message.content[0] else {
            panic!("expected a tool use, got {:?}", message.content[0]);
        };
        assert_eq!(signature.as_deref(), Some("sig-call"));
    }

    #[tokio::test]
    async fn an_unsigned_call_is_still_a_call() {
        // Every other model on this API, and every other provider.
        let (message, _) = replay(&[r#"{"candidates":[{"content":{"parts":[
                {"functionCall":{"name":"run_command","args":{}}}],"role":"model"}}]}"#])
        .await;
        let ContentBlock::ToolUse { signature, .. } = &message.content[0] else {
            panic!("expected a tool use, got {:?}", message.content[0]);
        };
        assert_eq!(*signature, None);
    }

    #[tokio::test]
    async fn thought_parts_land_in_a_thinking_block_not_the_answer() {
        let (message, _) = replay(&[r#"{"candidates":[{"content":{"parts":[
                {"text":"weighing it","thought":true,"thoughtSignature":"sig-1"},
                {"text":"the answer"}],"role":"model"}}]}"#])
        .await;
        assert_eq!(
            message.content[0],
            ContentBlock::Thinking {
                text: "weighing it".into(),
                signature: Some("sig-1".into()),
            }
        );
        // The reasoning must not leak into what the user reads.
        assert_eq!(message.text(), "the answer");
    }

    #[tokio::test]
    async fn reasoning_tokens_are_counted_as_output() {
        let (_, usage) = replay(&[
            r#"{"candidates":[{"content":{"parts":[{"text":"x"}],"role":"model"}}],
                "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":400}}"#,
        ])
        .await;
        assert_eq!(usage.output_tokens, 405);
    }

    #[test]
    fn the_key_rides_a_header_rather_than_the_query_string() {
        // A credential in a URL is a credential in every proxy log between here
        // and Google, and in every error report that quotes the request.
        let provider = GeminiProvider::new("gemini", DEFAULT_BASE_URL, Some("AIza-test".into()));
        let request = provider
            .authorize(provider.client.get(provider.url("/models")))
            .build()
            .expect("a buildable request");
        assert_eq!(request.headers()["x-goog-api-key"], "AIza-test");
        assert!(request.headers()["x-goog-api-key"].is_sensitive());
        assert!(!request.url().as_str().contains("AIza-test"));
    }

    #[test]
    fn the_streaming_route_names_the_model_in_the_path() {
        let provider = GeminiProvider::new("gemini", DEFAULT_BASE_URL, None);
        assert_eq!(
            provider.url("/models/gemini-2.5-pro:streamGenerateContent"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent"
        );
    }

    #[tokio::test]
    async fn declared_models_cost_no_request() {
        let provider = GeminiProvider::new("gemini", "http://127.0.0.1:1", None)
            .with_models(vec!["gemini-2.5-pro".into()]);
        assert_eq!(provider.models().await.unwrap()[0].id, "gemini-2.5-pro");
    }

    #[tokio::test]
    async fn a_models_capabilities_are_listed_once_and_then_remembered() {
        // Answering this here means listing every model the account can see,
        // and the agent loop asks once per iteration. Uncached, a ten-step turn
        // spends ten listings re-learning one number.
        let provider = GeminiProvider::new("gemini", "http://127.0.0.1:1", None);
        let first = provider.capabilities("gemini-2.5-pro").await.unwrap();
        assert!(provider.probed.read().await.contains_key("gemini-2.5-pro"));
        let second = provider.capabilities("gemini-2.5-pro").await.unwrap();
        assert_eq!(first.context_length, second.context_length);
    }

    #[tokio::test]
    async fn an_unreachable_listing_falls_back_rather_than_failing() {
        let provider = GeminiProvider::new("gemini", "http://127.0.0.1:1", None);
        let caps = provider.capabilities("gemini-2.5-pro").await.unwrap();
        assert!(caps.native_tools);
        assert_eq!(caps.context_length, 32_768);
    }
}
