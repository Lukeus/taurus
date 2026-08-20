//! Anthropic Messages API adapter.
//!
//! The third transport shape in this workspace, after Ollama's NDJSON and the
//! OpenAI adapter's SSE-with-a-string-for-tool-arguments. What is different here
//! is not the transport — it is SSE too — but that the wire format is the one
//! the normalized types were modelled on, so most of `convert.rs` is renaming
//! fields rather than restructuring turns.
//!
//! Two things this adapter does that no other one here can:
//!
//! - **Capabilities are probed rather than configured.** `/v1/models/{id}`
//!   reports the context window and a capability tree per model, so an Anthropic
//!   provider needs no `context_length` in `providers.json` and cannot be told
//!   the wrong one. Ollama is the only other backend that can answer this
//!   question about itself.
//! - **Prompt caching is on by default.** The system prompt and tool schemas are
//!   the fixed overhead `taurus usage` exists to report — re-sent on every
//!   iteration of every turn — and this is the one backend here that will serve
//!   them back at a tenth of the price for marking two breakpoints.

mod convert;
mod wire;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use taurus_provider::{
    Capabilities, ChatRequest, ModelInfo, Provider, ProviderError, Result, StopReason, StreamEvent,
    TokenUsage,
};

use wire::{BlockDelta, BlockStart, MessagesBody, ModelsResponse, StreamFrame};

pub use wire::DEFAULT_MAX_TOKENS;

/// The API version header. Required on every request, and pinned rather than
/// tracked: this is the contract the adapter was written against, and a newer
/// one could change shapes it parses.
const API_VERSION: &str = "2023-06-01";

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Where the Messages API lives on `api.anthropic.com`, and on anything
/// mirroring its route shape.
pub const DEFAULT_API_PREFIX: &str = "/v1";

/// The header this API reads the key from.
///
/// Not `Authorization: Bearer`, which is the mistake that produces a 401
/// indistinguishable from a wrong key. A gateway in front of it may want a
/// different one — see [`AnthropicProvider::with_api_key_header`].
pub const DEFAULT_API_KEY_HEADER: &str = "x-api-key";

/// What the models endpoint could not tell us.
///
/// Only reached when `/v1/models` is unavailable — a gateway that does not
/// proxy it, or a network that refused. Every Claude model calls tools
/// natively, which is why that one is not a guess.
#[derive(Clone, Copy, Debug)]
pub struct AnthropicCapabilities {
    pub vision: bool,
    pub context_length: u32,
}

impl Default for AnthropicCapabilities {
    fn default() -> Self {
        Self {
            vision: true,
            // Deliberately the smallest window any current Claude model has
            // rather than the largest. Guessing high means compacting far too
            // late and discovering it as a provider error mid-turn; guessing low
            // costs some unnecessary compaction and nothing else.
            context_length: 200_000,
        }
    }
}

/// How the request asks the model to reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Thinking {
    /// Send no `thinking` field, and let the model do what it does by default.
    ///
    /// The only setting valid on every model this API has served: the newer
    /// ones reason by default and the older ones do not, and neither rejects
    /// the request. Anything else is a preference that can be a 400.
    #[default]
    ModelDefault,
    /// `{"type": "adaptive"}` — the model decides how much to think per turn.
    Adaptive,
    /// `{"type": "disabled"}`. Rejected above a certain effort on some models,
    /// and on those it also makes the model more likely to narrate a tool call
    /// in prose instead of emitting one, so this is not the default.
    Disabled,
}

impl Thinking {
    fn to_wire(self) -> Option<serde_json::Value> {
        match self {
            Self::ModelDefault => None,
            Self::Adaptive => Some(serde_json::json!({"type": "adaptive"})),
            Self::Disabled => Some(serde_json::json!({"type": "disabled"})),
        }
    }

    /// Parses the value a config file may carry. Unknown words fall back to the
    /// safe setting rather than failing the provider.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "adaptive" | "on" | "true" => Self::Adaptive,
            "disabled" | "off" | "false" => Self::Disabled,
            _ => Self::ModelDefault,
        }
    }
}

pub struct AnthropicProvider {
    id: String,
    base_url: String,
    /// Already normalized: leading slash, no trailing one, possibly empty.
    api_prefix: String,
    api_key: Option<String>,
    /// Header the key goes in. Defaults to `x-api-key`.
    api_key_header: String,
    client: reqwest::Client,
    capabilities: AnthropicCapabilities,
    thinking: Thinking,
    /// Models the config named. Non-empty means `/v1/models` is never listed,
    /// which is what a gateway with no listing route needs.
    models: Vec<String>,
    /// One probe per model, kept for the life of this provider.
    ///
    /// Not an optimization but a correctness-adjacent fix: `capabilities` is
    /// asked once per iteration of the agent loop, because that is where
    /// compaction reads the context window. Probing each time would put a round
    /// trip to the API — and a rate-limit slot — in front of every model call
    /// in a turn, to re-learn a number that cannot change while the turn runs.
    probed: Arc<RwLock<HashMap<String, Capabilities>>>,
}

impl AnthropicProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_prefix: DEFAULT_API_PREFIX.to_string(),
            api_key,
            api_key_header: DEFAULT_API_KEY_HEADER.to_string(),
            client: reqwest::Client::new(),
            capabilities: AnthropicCapabilities::default(),
            thinking: Thinking::default(),
            models: Vec::new(),
            probed: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_thinking(mut self, thinking: Thinking) -> Self {
        self.thinking = thinking;
        self
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }

    /// Sends the key in a different header than `x-api-key`.
    ///
    /// For this API served through a gateway, which is a case the direct
    /// endpoint made easy to forget: an Azure APIM route reads
    /// `Ocp-Apim-Subscription-Key`, and the key the client holds is the
    /// gateway's rather than Anthropic's — the upstream one is supplied by the
    /// route's own policy and never leaves it.
    ///
    /// Exclusive, like the OpenAI adapter's: naming a header sends the key
    /// there and nowhere else. Sending both would hand a subscription key to
    /// Anthropic and an Anthropic key to the gateway, and one of the two would
    /// reject it.
    ///
    /// `None` keeps `x-api-key`, so a config that says nothing changes nothing.
    pub fn with_api_key_header(mut self, header: Option<impl Into<String>>) -> Self {
        if let Some(header) = header
            .map(Into::into)
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
        {
            self.api_key_header = header;
        }
        self
    }

    /// Moves the Messages API to a different path prefix.
    ///
    /// `/v1` is right for `api.anthropic.com` and for a gateway that mirrors
    /// its paths. It is not right for one that does not: an APIM API is
    /// published under a base path of its own, with operations usually mapped
    /// straight onto `/messages`, so the `/v1` this used to force produced a
    /// 404 on a route that was configured perfectly well.
    ///
    /// An empty string is a legitimate answer — it means the routes sit
    /// directly under the base URL.
    ///
    /// `None` keeps the default.
    pub fn with_api_prefix(mut self, prefix: Option<impl AsRef<str>>) -> Self {
        if let Some(prefix) = prefix {
            self.api_prefix = normalize_prefix(prefix.as_ref());
        }
        self
    }

    /// Fallback values for when the models endpoint cannot be reached.
    pub fn with_fallback_capabilities(mut self, capabilities: AnthropicCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base_url, self.api_prefix, path)
    }

    /// Adds the two headers every request needs.
    ///
    /// `x-api-key` by default, not `Authorization: Bearer` — this API is the
    /// reason the OpenAI adapter grew a configurable header, and getting it
    /// wrong is a 401 that reads exactly like a bad key. A gateway in front of
    /// it can want another name again, which is why this is now a setting here
    /// too rather than a constant.
    ///
    /// `anthropic-version` goes on regardless of where the key rides. A gateway
    /// that injects its own is unharmed by receiving the same value, and one
    /// that passes the request straight through needs it.
    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let builder = builder.header("anthropic-version", API_VERSION);
        let Some(key) = &self.api_key else {
            return builder;
        };
        // Built by hand so it can be marked sensitive: reqwest renders headers
        // with `{:?}` when tracing a request, and a key in a debug log is a
        // leaked credential.
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
        builder.header(self.api_key_header.as_str(), value)
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

    /// One model's own report of itself, or `None` if the endpoint will not say.
    async fn probe(&self, model: &str) -> Option<wire::ModelEntry> {
        let response = self
            .authorize(self.client.get(self.url(&format!("/models/{model}"))))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json().await.ok()
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
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
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        let response = self.check_status(response).await?;
        let models: ModelsResponse = response.json().await.map_err(|e| self.unreachable(e))?;
        Ok(models
            .data
            .into_iter()
            .map(|m| ModelInfo {
                display_name: m.display_name.unwrap_or_else(|| m.id.clone()),
                context_length: m.max_input_tokens,
                id: m.id,
            })
            .collect())
    }

    async fn capabilities(&self, model: &str) -> Result<Capabilities> {
        if let Some(cached) = self.probed.read().await.get(model) {
            return Ok(*cached);
        }

        let probed = self.probe(model).await;
        let capabilities = Capabilities {
            // Not probed, because it is not in question: every Claude model
            // this API serves calls tools natively, and a prompted fallback
            // here would be a worse implementation of a feature that works.
            native_tools: true,
            vision: probed
                .as_ref()
                .and_then(|m| m.vision())
                .unwrap_or(self.capabilities.vision),
            thinking: probed
                .as_ref()
                .and_then(|m| m.thinking())
                .unwrap_or(self.thinking != Thinking::Disabled),
            context_length: probed
                .as_ref()
                .and_then(|m| m.max_input_tokens)
                .unwrap_or(self.capabilities.context_length),
        };

        // Cached even when the probe failed and these are the fallbacks. A
        // backend that would not answer once will not answer forty times in
        // the same turn either, and retrying is a stall per iteration.
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
        let body = MessagesBody::from_request(&request, self.thinking.to_wire());
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(StopReason::Canceled),
            r = self.authorize(self.client.post(self.url("/messages")).json(&body)).send() => {
                self.check_status(r.map_err(|e| self.unreachable(e))?).await?
            }
        };

        let mut reader = SseReader::new(response.bytes_stream());
        // Blocks are addressed by index, and only a tool-use block needs its id
        // carried: the deltas that follow name the index, not the call.
        let mut open_tools: HashMap<u32, String> = HashMap::new();
        let mut usage = TokenUsage::default();
        let mut stop_reason = None;

        loop {
            let data = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(StopReason::Canceled),
                next = reader.next_event() => next,
            };
            let Some(data) = data.map_err(|e| self.unreachable(e))? else {
                break;
            };

            let frame: StreamFrame = match serde_json::from_str(&data) {
                Ok(frame) => frame,
                Err(e) => {
                    warn!(error = %e, "skipping malformed SSE frame");
                    continue;
                }
            };

            match frame {
                StreamFrame::MessageStart { message } => {
                    if let Some(u) = message.usage {
                        usage.input_tokens = u.input_total();
                    }
                }

                StreamFrame::ContentBlockStart {
                    index,
                    content_block,
                } => match content_block {
                    BlockStart::ToolUse { id, name } => {
                        open_tools.insert(index, id.clone());
                        send(&tx, StreamEvent::ToolUseStart { id, name }).await?;
                    }
                    // Opened with an empty delta rather than waiting for text.
                    // On models that return their reasoning summarized away, a
                    // thinking block carries a signature and no text at all —
                    // and a signature with no block to attach it to is dropped,
                    // which makes the next request's replay illegal.
                    BlockStart::Thinking | BlockStart::RedactedThinking => {
                        send(
                            &tx,
                            StreamEvent::ThinkingDelta {
                                text: String::new(),
                            },
                        )
                        .await?;
                    }
                    BlockStart::Text | BlockStart::Other => {}
                },

                StreamFrame::ContentBlockDelta { index, delta } => match delta {
                    BlockDelta::TextDelta { text } => {
                        send(&tx, StreamEvent::TextDelta { text }).await?;
                    }
                    BlockDelta::ThinkingDelta { thinking } => {
                        send(&tx, StreamEvent::ThinkingDelta { text: thinking }).await?;
                    }
                    BlockDelta::SignatureDelta { signature } => {
                        send(&tx, StreamEvent::ThinkingSignature { signature }).await?;
                    }
                    BlockDelta::InputJsonDelta { partial_json } => {
                        let Some(id) = open_tools.get(&index).cloned() else {
                            warn!(index, "tool input fragment with no opening frame");
                            continue;
                        };
                        send(
                            &tx,
                            StreamEvent::ToolUseInputDelta {
                                id,
                                json: partial_json,
                            },
                        )
                        .await?;
                    }
                    BlockDelta::Other => {}
                },

                StreamFrame::ContentBlockStop { index } => {
                    if let Some(id) = open_tools.remove(&index) {
                        send(&tx, StreamEvent::ToolUseEnd { id }).await?;
                    }
                }

                StreamFrame::MessageDelta { delta, usage: u } => {
                    if let Some(u) = u {
                        usage.output_tokens = u.output_tokens.unwrap_or(usage.output_tokens);
                    }
                    stop_reason = delta.stop_reason.or(stop_reason);
                }

                StreamFrame::MessageStop => break,

                // A 200 was already sent, so this cannot arrive as a status
                // code. Surfaced as an API error so the agent loop's retry
                // logic sees an overload the same way it would a 529.
                StreamFrame::Error { error } => {
                    return Err(ProviderError::Api {
                        provider: self.id.clone(),
                        status: status_for(&error.kind),
                        body: format!("{}: {}", error.kind, error.message),
                    });
                }

                StreamFrame::Other => {}
            }
        }

        // A block left open by a truncated stream still has to be closed, or the
        // accumulator holds a tool call with no input.
        for id in open_tools.into_values() {
            send(&tx, StreamEvent::ToolUseEnd { id }).await?;
        }
        send(&tx, StreamEvent::Usage { usage }).await?;

        Ok(match stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("stop_sequence") => StopReason::StopSequence,
            // `refusal` and `pause_turn` both end the turn with something to
            // say. Neither is a tool call and neither is an error the harness
            // can act on differently, so both read as a finished turn.
            _ => StopReason::EndTurn,
        })
    }
}

/// The status an in-stream error would have carried had it arrived as one.
///
/// Only so [`ProviderError::is_transient`] classifies it the way the same
/// failure would be classified before the stream opened — an overload is worth
/// retrying whether it is discovered at connect time or ten frames in.
fn status_for(kind: &str) -> u16 {
    match kind {
        "overloaded_error" => 529,
        "rate_limit_error" => 429,
        "api_error" => 500,
        "authentication_error" => 401,
        "permission_error" => 403,
        "not_found_error" => 404,
        _ => 400,
    }
}

/// A path prefix as `url` needs it: one leading slash, no trailing one, and
/// empty for a gateway that mounts the routes at its own root.
fn normalize_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

async fn send(tx: &mpsc::Sender<StreamEvent>, event: StreamEvent) -> Result<()> {
    tx.send(event).await.map_err(|_| ProviderError::Canceled)
}

/// Minimal SSE reader: yields the payload of each `data:` line.
///
/// The `event:` lines are ignored on purpose — every frame repeats its type in
/// the JSON body, so parsing both would be two sources of the same truth that
/// can disagree.
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

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new("anthropic", DEFAULT_BASE_URL, Some("sk-test".into()))
    }

    /// Feeds a recorded frame sequence through the adapter's own decoding and
    /// the shared accumulator, so a test asserts the `Message` a turn produces
    /// rather than the events it happened to emit on the way there.
    async fn replay(frames: &[&str]) -> (taurus_provider::Message, TokenUsage) {
        let (tx, mut rx) = mpsc::channel(64);
        let mut open_tools: HashMap<u32, String> = HashMap::new();
        let mut usage = TokenUsage::default();

        for raw in frames {
            let frame: StreamFrame = serde_json::from_str(raw).expect(raw);
            match frame {
                StreamFrame::MessageStart { message } => {
                    if let Some(u) = message.usage {
                        usage.input_tokens = u.input_total();
                    }
                }
                StreamFrame::ContentBlockStart {
                    index,
                    content_block,
                } => match content_block {
                    BlockStart::ToolUse { id, name } => {
                        open_tools.insert(index, id.clone());
                        tx.send(StreamEvent::ToolUseStart { id, name })
                            .await
                            .unwrap();
                    }
                    BlockStart::Thinking | BlockStart::RedactedThinking => {
                        tx.send(StreamEvent::ThinkingDelta {
                            text: String::new(),
                        })
                        .await
                        .unwrap();
                    }
                    _ => {}
                },
                StreamFrame::ContentBlockDelta { index, delta } => match delta {
                    BlockDelta::TextDelta { text } => {
                        tx.send(StreamEvent::TextDelta { text }).await.unwrap()
                    }
                    BlockDelta::ThinkingDelta { thinking } => tx
                        .send(StreamEvent::ThinkingDelta { text: thinking })
                        .await
                        .unwrap(),
                    BlockDelta::SignatureDelta { signature } => tx
                        .send(StreamEvent::ThinkingSignature { signature })
                        .await
                        .unwrap(),
                    BlockDelta::InputJsonDelta { partial_json } => {
                        let id = open_tools.get(&index).cloned().unwrap();
                        tx.send(StreamEvent::ToolUseInputDelta {
                            id,
                            json: partial_json,
                        })
                        .await
                        .unwrap();
                    }
                    BlockDelta::Other => {}
                },
                StreamFrame::ContentBlockStop { index } => {
                    if let Some(id) = open_tools.remove(&index) {
                        tx.send(StreamEvent::ToolUseEnd { id }).await.unwrap();
                    }
                }
                StreamFrame::MessageDelta { usage: Some(u), .. } => {
                    usage.output_tokens = u.output_tokens.unwrap_or(0);
                }
                _ => {}
            }
        }
        drop(tx);

        let mut acc = StreamAccumulator::new();
        while let Some(event) = rx.recv().await {
            acc.push(event);
        }
        let (message, _, _) = acc.finish();
        (message, usage)
    }

    #[tokio::test]
    async fn a_text_turn_reassembles_into_one_block() {
        let (message, usage) = replay(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            r#"{"type":"message_stop"}"#,
        ])
        .await;
        assert_eq!(message.text(), "Hello");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn a_tool_call_assembles_from_input_fragments() {
        let (message, _) = replay(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ])
        .await;
        let (id, name, input) = message.tool_uses().next().expect("a tool call");
        assert_eq!((id, name), ("toolu_1", "read_file"));
        assert_eq!(input, &serde_json::json!({"path": "a.txt"}));
    }

    #[tokio::test]
    async fn a_thinking_block_keeps_its_signature() {
        // The load-bearing one. A turn that reasoned and then called a tool is
        // only legal on the next request with its thinking replayed signed, so
        // a signature that does not survive the stream is a 400 one turn later.
        let (message, _) = replay(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"weighing it"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-abc"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"done"}}"#,
        ])
        .await;
        assert_eq!(
            message.content[0],
            ContentBlock::Thinking {
                text: "weighing it".into(),
                signature: Some("sig-abc".into()),
            }
        );
        assert_eq!(message.text(), "done");
    }

    #[tokio::test]
    async fn reasoning_returned_without_text_still_produces_a_signed_block() {
        // Current models return their thinking summarized away by default: the
        // block carries a signature and an empty string. Waiting for text
        // before opening the block would drop the signature on the floor.
        let (message, _) = replay(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-xyz"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ])
        .await;
        assert_eq!(
            message.content[0],
            ContentBlock::Thinking {
                text: String::new(),
                signature: Some("sig-xyz".into()),
            }
        );
    }

    #[tokio::test]
    async fn two_tool_calls_in_one_turn_stay_separate() {
        // Blocks are addressed by index here, not by an id repeated on every
        // fragment, so interleaved indices must not cross-contaminate.
        let (message, _) = replay(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"a"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":1}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t2","name":"b"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"y\":2}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
        ])
        .await;
        let calls: Vec<_> = message.tool_uses().collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].2, &serde_json::json!({"x": 1}));
        assert_eq!(calls[1].2, &serde_json::json!({"y": 2}));
    }

    #[test]
    fn the_key_rides_x_api_key_and_never_authorization() {
        // This API is the reason the OpenAI adapter grew a configurable header.
        // A bearer token here is a 401 that reads exactly like a bad key.
        let provider = provider();
        let headers = provider
            .authorize(provider.client.get("http://x"))
            .build()
            .expect("a buildable request")
            .headers()
            .clone();
        assert_eq!(headers["x-api-key"], "sk-test");
        assert_eq!(headers["anthropic-version"], API_VERSION);
        assert!(!headers.contains_key("authorization"));
    }

    /// The headers `authorize` actually puts on a request.
    fn auth_headers(header: Option<&str>) -> reqwest::header::HeaderMap {
        let provider =
            AnthropicProvider::new("anthropic", DEFAULT_BASE_URL, Some("sk-test".into()))
                .with_api_key_header(header);
        provider
            .authorize(provider.client.get("http://x"))
            .build()
            .expect("a buildable request")
            .headers()
            .clone()
    }

    #[test]
    fn a_gateway_can_take_the_key_in_a_header_of_its_own() {
        // An Azure APIM route in front of this API reads its own subscription
        // key; the Anthropic key belongs to the route's policy and never
        // reaches the client. Before this, the header was a constant and there
        // was nowhere for that key to ride.
        let headers = auth_headers(Some("Ocp-Apim-Subscription-Key"));
        assert_eq!(headers["ocp-apim-subscription-key"], "sk-test");
        // Exclusive: sending both would hand a subscription key to Anthropic
        // and an Anthropic key to the gateway, and one of the two would reject
        // it.
        assert!(!headers.contains_key("x-api-key"));
        // The version header goes on regardless of where the key rides.
        assert_eq!(headers["anthropic-version"], API_VERSION);
    }

    #[test]
    fn a_header_that_says_nothing_leaves_the_default_alone() {
        for named in [None, Some(""), Some("   ")] {
            let headers = auth_headers(named);
            assert_eq!(headers["x-api-key"], "sk-test", "for {named:?}");
        }
    }

    #[test]
    fn a_gateway_can_publish_the_routes_under_its_own_path() {
        // The `/v1` this used to force produced a 404 on an APIM route that was
        // configured perfectly well: an API published there has a base path of
        // its own, and its operations usually map straight onto `/messages`.
        let gateway = AnthropicProvider::new("apim", "https://gw.azure-api.net/claude", None)
            .with_api_prefix(Some(""));
        assert_eq!(
            gateway.url("/messages"),
            "https://gw.azure-api.net/claude/messages"
        );

        let prefixed = AnthropicProvider::new("apim", "https://gw.azure-api.net", None)
            .with_api_prefix(Some("anthropic/v1"));
        assert_eq!(
            prefixed.url("/messages"),
            "https://gw.azure-api.net/anthropic/v1/messages"
        );
        // Written with or without slashes, it lands the same way.
        let slashed = AnthropicProvider::new("apim", "https://gw.azure-api.net", None)
            .with_api_prefix(Some("/anthropic/v1/"));
        assert_eq!(slashed.url("/messages"), prefixed.url("/messages"));
    }

    #[test]
    fn a_prefix_that_says_nothing_leaves_the_default_alone() {
        assert_eq!(
            provider().url("/messages"),
            format!("{DEFAULT_BASE_URL}/v1/messages")
        );
        assert_eq!(
            AnthropicProvider::new("anthropic", DEFAULT_BASE_URL, None)
                .with_api_prefix(None::<&str>)
                .url("/messages"),
            format!("{DEFAULT_BASE_URL}/v1/messages")
        );
    }

    #[test]
    fn the_key_is_marked_sensitive_so_it_stays_out_of_debug_output() {
        let provider = provider();
        let headers = provider
            .authorize(provider.client.get("http://x"))
            .build()
            .unwrap()
            .headers()
            .clone();
        assert!(headers["x-api-key"].is_sensitive());
        assert!(!format!("{headers:?}").contains("sk-test"));
    }

    #[test]
    fn an_in_stream_overload_is_classified_as_retryable() {
        // It arrives after a 200, so it cannot surface as a status code. If it
        // did not carry one the agent loop would give up on a failure that a
        // retry fixes.
        let error = ProviderError::Api {
            provider: "anthropic".into(),
            status: status_for("overloaded_error"),
            body: "overloaded".into(),
        };
        assert!(error.is_transient());

        let fatal = ProviderError::Api {
            provider: "anthropic".into(),
            status: status_for("invalid_request_error"),
            body: "bad".into(),
        };
        assert!(!fatal.is_transient());
    }

    #[test]
    fn thinking_settings_parse_leniently_and_default_to_the_safe_one() {
        assert_eq!(Thinking::parse("adaptive"), Thinking::Adaptive);
        assert_eq!(Thinking::parse("OFF"), Thinking::Disabled);
        // A word nobody recognizes must not take the provider down.
        assert_eq!(Thinking::parse("whatever"), Thinking::ModelDefault);
        assert!(Thinking::ModelDefault.to_wire().is_none());
    }

    #[tokio::test]
    async fn declared_models_cost_no_request() {
        // The base URL is unroutable: reaching for `/v1/models` at all would
        // fail rather than answer.
        let provider = AnthropicProvider::new("anthropic", "http://127.0.0.1:1", None)
            .with_models(vec!["claude-opus-5".into()]);
        let models = provider
            .models()
            .await
            .expect("declared models cannot fail");
        assert_eq!(models[0].id, "claude-opus-5");
    }

    #[tokio::test]
    async fn an_unreachable_models_endpoint_falls_back_rather_than_failing() {
        // A gateway that does not proxy `/v1/models` must still be usable.
        let provider = AnthropicProvider::new("anthropic", "http://127.0.0.1:1", None);
        let caps = provider.capabilities("claude-opus-5").await.unwrap();
        assert!(caps.native_tools);
        assert_eq!(caps.context_length, 200_000);
    }

    #[tokio::test]
    async fn a_models_capabilities_are_probed_once_and_then_remembered() {
        // `capabilities` is asked once per iteration of the agent loop, so an
        // uncached probe puts a round trip and a rate-limit slot in front of
        // every model call in a turn. The unroutable base URL makes the first
        // call slow and the rest instant; what is asserted is that the answer
        // is stable and cheap, not that a network call did not happen.
        let provider = AnthropicProvider::new("anthropic", "http://127.0.0.1:1", None);
        let first = provider.capabilities("claude-opus-5").await.unwrap();
        assert!(provider.probed.read().await.contains_key("claude-opus-5"));
        let second = provider.capabilities("claude-opus-5").await.unwrap();
        assert_eq!(first.context_length, second.context_length);
    }

    #[test]
    fn the_request_body_carries_a_ceiling_because_the_api_requires_one() {
        let request = ChatRequest::new("claude-opus-5", vec![Message::user("hi")]);
        let body = MessagesBody::from_request(&request, None);
        assert!(body.max_tokens > 0);
    }
}
