//! OpenAI-compatible adapter.
//!
//! Covers OpenAI itself plus the many servers that speak its API: vLLM, LM
//! Studio, llama.cpp's server, OpenRouter, Groq, Together. The differences from
//! Ollama are structural rather than cosmetic — SSE instead of NDJSON, tool
//! arguments as a *string* assembled across deltas instead of an object, and
//! index-keyed tool calls with no id on continuation frames.
//!
//! This crate is the load-bearing proof that [`taurus_provider::Provider`] is
//! not Ollama-shaped: adding it required no change to `taurus-core`.

mod convert;
mod wire;

use std::collections::HashMap;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use taurus_provider::prompted::{PromptedScanner, PromptedTools};
use taurus_provider::{
    Capabilities, ChatRequest, ModelInfo, Provider, ProviderError, Result, StopReason, StreamEvent,
    TokenUsage,
};

use wire::{ChatBody, ModelsResponse, StreamChunk};

/// How a model's capabilities are determined.
///
/// OpenAI-compatible servers have no capability endpoint — `/v1/models`
/// returns ids and nothing else — so unlike Ollama this cannot be probed and
/// must be configured.
#[derive(Clone, Copy, Debug)]
pub struct OpenAiCapabilities {
    pub native_tools: bool,
    pub vision: bool,
    pub context_length: u32,
}

impl Default for OpenAiCapabilities {
    fn default() -> Self {
        // Hosted OpenAI-compatible endpoints support tools; a self-hosted
        // server serving a base model may not, which is what the config
        // override is for.
        Self {
            native_tools: true,
            vision: false,
            context_length: 128_000,
        }
    }
}

pub struct OpenAiProvider {
    id: String,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
    capabilities: OpenAiCapabilities,
}

impl OpenAiProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        capabilities: OpenAiCapabilities,
    ) -> Self {
        Self {
            id: id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            client: reqwest::Client::new(),
            capabilities,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => builder.bearer_auth(key),
            None => builder,
        }
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
        if status.as_u16() == 401 || status.as_u16() == 403 {
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
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        let response = self
            .authorize(self.client.get(self.url("/v1/models")))
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        let response = self.check_status(response).await?;
        let models: ModelsResponse = response.json().await.map_err(|e| self.unreachable(e))?;
        Ok(models
            .data
            .into_iter()
            .map(|m| ModelInfo {
                display_name: m.id.clone(),
                id: m.id,
                context_length: None,
            })
            .collect())
    }

    async fn capabilities(&self, _model: &str) -> Result<Capabilities> {
        Ok(Capabilities {
            native_tools: self.capabilities.native_tools,
            vision: self.capabilities.vision,
            // No OpenAI-compatible endpoint exposes reasoning as a separate
            // stream field, so thinking is always folded into text here.
            thinking: false,
            context_length: self.capabilities.context_length,
        })
    }

    async fn stream(
        &self,
        mut request: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> Result<StopReason> {
        let prompted = !self.capabilities.native_tools && !request.tools.is_empty();
        if prompted {
            PromptedTools::rewrite(&mut request);
        }

        let body = ChatBody::from_request(&request);
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(StopReason::Canceled),
            r = self.authorize(self.client.post(self.url("/v1/chat/completions")).json(&body)).send() => {
                self.check_status(r.map_err(|e| self.unreachable(e))?).await?
            }
        };

        let mut reader = SseReader::new(response.bytes_stream());
        let mut scanner = prompted.then(PromptedScanner::new);
        // Tool calls arrive as fragments keyed by index; the id and name only
        // appear on the first fragment.
        let mut open_calls: HashMap<u32, String> = HashMap::new();
        let mut usage = TokenUsage::default();
        let mut finish_reason = None;

        loop {
            let data = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(StopReason::Canceled),
                next = reader.next_event() => next,
            };
            let Some(data) = data.map_err(|e| self.unreachable(e))? else {
                break;
            };
            if data == "[DONE]" {
                break;
            }

            let chunk: StreamChunk = match serde_json::from_str(&data) {
                Ok(chunk) => chunk,
                Err(e) => {
                    warn!(error = %e, "skipping malformed SSE chunk");
                    continue;
                }
            };

            if let Some(u) = chunk.usage {
                usage = TokenUsage {
                    input_tokens: u.prompt_tokens.unwrap_or(0),
                    output_tokens: u.completion_tokens.unwrap_or(0),
                };
            }

            let Some(choice) = chunk.choices.into_iter().next() else {
                continue;
            };
            if let Some(reason) = choice.finish_reason {
                finish_reason = Some(reason);
            }

            let Some(delta) = choice.delta else { continue };

            if let Some(content) = delta.content.filter(|c| !c.is_empty()) {
                match scanner.as_mut() {
                    Some(scanner) => {
                        for event in scanner.feed(&content) {
                            send(&tx, event).await?;
                        }
                    }
                    None => send(&tx, StreamEvent::TextDelta { text: content }).await?,
                }
            }

            // Some servers expose reasoning models' scratchpad here.
            if let Some(reasoning) = delta.reasoning_content.filter(|r| !r.is_empty()) {
                send(&tx, StreamEvent::ThinkingDelta { text: reasoning }).await?;
            }

            for call in delta.tool_calls {
                let index = call.index.unwrap_or(0);
                if let Some(id) = call.id.filter(|id| !id.is_empty()) {
                    // First fragment: open the call.
                    let name = call
                        .function
                        .as_ref()
                        .and_then(|f| f.name.clone())
                        .unwrap_or_default();
                    open_calls.insert(index, id.clone());
                    send(&tx, StreamEvent::ToolUseStart { id, name }).await?;
                }
                let Some(id) = open_calls.get(&index).cloned() else {
                    warn!(index, "tool call fragment with no opening frame");
                    continue;
                };
                if let Some(args) = call.function.and_then(|f| f.arguments) {
                    if !args.is_empty() {
                        send(&tx, StreamEvent::ToolUseInputDelta { id, json: args }).await?;
                    }
                }
            }
        }

        let mut saw_tool_call = !open_calls.is_empty();
        for id in open_calls.into_values() {
            send(&tx, StreamEvent::ToolUseEnd { id }).await?;
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
            match finish_reason.as_deref() {
                Some("length") => StopReason::MaxTokens,
                Some("tool_calls") => StopReason::ToolUse,
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
                // Blank lines separate events; comment lines start with ':'.
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
