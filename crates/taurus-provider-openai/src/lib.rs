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
    Capabilities, ChatRequest, ModelInfo, Provider, ProviderError, RerankScore, Result, StopReason,
    StreamEvent, TokenUsage,
};

use wire::{
    ChatBody, EmbedBody, EmbedResponse, ModelsResponse, RerankBody, RerankResponse, StreamChunk,
};

/// Path the OpenAI routes live under on almost every server.
pub const DEFAULT_API_PREFIX: &str = "/v1";

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
        // Hosted OpenAI-compatible endpoints support tools and take images; a
        // self-hosted server serving a base model may do neither, which is what
        // the config overrides are for. Vision defaults on because every model
        // the hosted API has shipped since gpt-4o reads images, and defaulting
        // it off refused screenshots on the provider most people point at.
        Self {
            native_tools: true,
            vision: true,
            context_length: 128_000,
        }
    }
}

/// A model the config named, rather than one the server offered.
///
/// The overrides are per model because a single gateway routinely fronts
/// models that do not share a context window, tool support, or vision, and
/// `/v1/models` reports none of them. Unset means "whatever the provider says".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: String,
    pub display_name: Option<String>,
    pub context_length: Option<u32>,
    pub native_tools: Option<bool>,
    pub vision: Option<bool>,
}

impl ModelSpec {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }
}

pub struct OpenAiProvider {
    id: String,
    base_url: String,
    /// Already normalized: leading slash, no trailing one, possibly empty.
    api_prefix: String,
    api_key: Option<String>,
    /// Header the key goes in. `None` means bearer auth.
    api_key_header: Option<String>,
    client: reqwest::Client,
    capabilities: OpenAiCapabilities,
    /// Declared models. Non-empty means `/v1/models` is never called.
    models: Vec<ModelSpec>,
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
            api_prefix: DEFAULT_API_PREFIX.to_string(),
            api_key,
            api_key_header: None,
            client: reqwest::Client::new(),
            capabilities,
            models: Vec::new(),
        }
    }

    /// Declares the models this endpoint serves, instead of asking it.
    ///
    /// A gateway need not expose `/v1/models` at all, and plenty of the ones
    /// that do answer with an inventory rather than an entitlement — every
    /// model the vendor sells, including the ones this key cannot call. Naming
    /// them here replaces the listing outright: what is declared is what the
    /// picker offers, and no request is made to find out.
    ///
    /// An empty list changes nothing, so a config that says nothing still asks.
    pub fn with_models(mut self, models: Vec<ModelSpec>) -> Self {
        self.models = models;
        self
    }

    /// What the config said about one model, if it said anything.
    fn declared(&self, model: &str) -> Option<&ModelSpec> {
        self.models.iter().find(|m| m.id == model)
    }

    /// Sends the key in a named header instead of as a bearer token.
    ///
    /// OpenAI and everything imitating it want `Authorization: Bearer <key>`,
    /// which is the default and what `None` preserves. Azure does not: Azure
    /// OpenAI reads `api-key`, and an Azure APIM gateway reads
    /// `Ocp-Apim-Subscription-Key` — both bare, with no scheme prefix.
    ///
    /// So a named header carries the key raw. That one rule covers every
    /// gateway worth naming, including one that wants a bare `Authorization`
    /// with no `Bearer`, which is why there is no separate prefix setting.
    pub fn with_api_key_header(mut self, header: Option<impl Into<String>>) -> Self {
        self.api_key_header = header
            .map(Into::into)
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty());
        self
    }

    /// Moves the OpenAI routes to a different path prefix.
    ///
    /// `/v1` covers OpenAI itself and nearly every server that imitates it.
    /// The exception worth naming is OpenVINO Model Server, which served these
    /// routes under `/v3` until 2026.3 added `/v1` as an alias. A server behind
    /// a reverse proxy that mounts the API somewhere else needs this too.
    ///
    /// `None` keeps the default, so a config that says nothing changes nothing.
    pub fn with_api_prefix(mut self, prefix: Option<impl AsRef<str>>) -> Self {
        if let Some(prefix) = prefix {
            self.api_prefix = normalize_prefix(prefix.as_ref());
        }
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.base_url, self.api_prefix, path)
    }

    // The routes, named rather than spelled out at the call sites, so a
    // test asserts the same string the request uses. Written inline they were
    // passed to `url` still carrying the `/v1` the prefix now supplies, and
    // every test still passed.
    fn models_url(&self) -> String {
        self.url("/models")
    }

    fn chat_url(&self) -> String {
        self.url("/chat/completions")
    }

    fn rerank_url(&self) -> String {
        self.url("/rerank")
    }

    fn embeddings_url(&self) -> String {
        self.url("/embeddings")
    }

    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let Some(key) = &self.api_key else {
            return builder;
        };
        match &self.api_key_header {
            None => builder.bearer_auth(key),
            Some(name) => {
                // Built by hand rather than passed as a &str so it can be
                // marked sensitive: reqwest prints headers in its `{:?}`, and a
                // subscription key in a debug log is a leaked credential.
                let mut value = match reqwest::header::HeaderValue::from_str(key) {
                    Ok(value) => value,
                    // A key with a newline or a non-ASCII byte in it cannot be
                    // sent. Dropping the header produces a 401 the user can
                    // act on; panicking on their config would not.
                    Err(_) => {
                        warn!(
                            provider = %self.id,
                            "the API key has characters an HTTP header cannot carry; sending none"
                        );
                        return builder;
                    }
                };
                value.set_sensitive(true);
                builder.header(name, value)
            }
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
        // Declared beats discovered, and skips the round trip entirely. This
        // is also the only path that works on a gateway with no listing route.
        if !self.models.is_empty() {
            return Ok(self
                .models
                .iter()
                .map(|m| ModelInfo {
                    id: m.id.clone(),
                    display_name: m.display_name.clone().unwrap_or_else(|| m.id.clone()),
                    context_length: m.context_length,
                })
                .collect());
        }

        let response = self
            .authorize(self.client.get(self.models_url()))
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

    async fn capabilities(&self, model: &str) -> Result<Capabilities> {
        // Per model where the config bothered to say, per provider otherwise.
        // The difference matters most for context length: one gateway fronting
        // gpt-4o and an 8k local model compacts far too late for the second if
        // both are told they have 128k.
        let declared = self.declared(model);
        Ok(Capabilities {
            native_tools: declared
                .and_then(|m| m.native_tools)
                .unwrap_or(self.capabilities.native_tools),
            vision: declared
                .and_then(|m| m.vision)
                .unwrap_or(self.capabilities.vision),
            // No OpenAI-compatible endpoint exposes reasoning as a separate
            // stream field, so thinking is always folded into text here.
            thinking: false,
            context_length: declared
                .and_then(|m| m.context_length)
                .unwrap_or(self.capabilities.context_length),
        })
    }

    /// Embeds against `/v1/embeddings`.
    ///
    /// The one OpenAI route this adapter had not implemented, which meant
    /// `search_code` was unavailable to everybody not running Ollama — on a
    /// backend that has served embeddings the whole time. llama.cpp,
    /// LM Studio, vLLM, text-embeddings-inference, Together and OpenAI itself
    /// all answer here.
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .authorize(self.client.post(self.embeddings_url()).json(&EmbedBody {
                model,
                input: inputs,
            }))
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        let response = self.check_status(response).await.map_err(|e| match e {
            // The common misconfiguration is naming a *chat* model here. The
            // API says "not found" for that, which reads as a typo rather than
            // as the two namespaces being separate.
            ProviderError::Api { status: 404, .. } => ProviderError::Protocol(format!(
                "{} has no embedding model called '{model}'. Embedding models are a separate \
                 namespace from chat models — `text-embedding-3-small` on OpenAI, or whatever \
                 your server loaded with `--embedding`.",
                self.id
            )),
            other => other,
        })?;
        let embedded: EmbedResponse = response.json().await.map_err(|e| self.unreachable(e))?;

        // A backend that answered with a different number of vectors than it
        // was given texts has broken the only contract that makes the result
        // usable: position is the only thing tying a vector to its chunk.
        if embedded.data.len() != inputs.len() {
            return Err(ProviderError::Protocol(format!(
                "asked {} for {} embeddings and got {}",
                self.id,
                inputs.len(),
                embedded.data.len()
            )));
        }

        // Placed by the index the server reported rather than by arrival order.
        // The order is documented, and a server that got it wrong would attach
        // every vector to the wrong chunk — a search that confidently returns
        // the wrong file, which is far harder to notice than one that fails.
        let mut vectors: Vec<Option<Vec<f32>>> = vec![None; inputs.len()];
        for item in embedded.data {
            let Some(slot) = vectors.get_mut(item.index) else {
                return Err(ProviderError::Protocol(format!(
                    "{} returned an embedding for input {} of {}",
                    self.id,
                    item.index,
                    inputs.len()
                )));
            };
            *slot = Some(item.embedding);
        }
        vectors
            .into_iter()
            .enumerate()
            .map(|(n, vector)| {
                vector.ok_or_else(|| {
                    ProviderError::Protocol(format!(
                        "{} returned no embedding for input {n}",
                        self.id
                    ))
                })
            })
            .collect()
    }

    /// Reranks against the Cohere-shaped `/rerank` route.
    ///
    /// On this adapter rather than Ollama's because this is where the servers
    /// that serve it are reached: llama.cpp started with `--reranking`,
    /// text-embeddings-inference, and the hosted rerankers all sit behind an
    /// OpenAI-compatible base URL, and Ollama has no reranking route at all.
    async fn rerank(
        &self,
        model: &str,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<RerankScore>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let body = RerankBody {
            model,
            query,
            documents,
            top_n: documents.len(),
            return_documents: false,
        };
        let response = self
            .authorize(self.client.post(self.rerank_url()).json(&body))
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        let response = self.check_status(response).await.map_err(|e| match e {
            // A 404 here is the common misconfiguration and it is worth naming,
            // because the generic form of it — "returned 404" against a URL the
            // user never typed — reads as a broken Taurus rather than a server
            // that was started without the flag that serves this route.
            ProviderError::Api { status: 404, .. } => ProviderError::Protocol(format!(
                "{} has no reranking route at {}. A llama.cpp server serves one only when \
                 started with `--reranking`; most other backends serve none at all.",
                self.id,
                self.rerank_url()
            )),
            other => other,
        })?;
        let scored: RerankResponse = response.json().await.map_err(|e| self.unreachable(e))?;

        // Position is the only thing tying a score back to a document, so an
        // index outside the slice is not a score that can be used — and taking
        // it on trust would panic in the caller's indexing rather than fail
        // here, where the provider that produced it can be named.
        if let Some(bad) = scored.results.iter().find(|r| r.index >= documents.len()) {
            return Err(ProviderError::Protocol(format!(
                "{} scored document {} of {}, which was not sent",
                self.id,
                bad.index,
                documents.len()
            )));
        }

        Ok(scored
            .results
            .into_iter()
            .map(|r| RerankScore {
                index: r.index,
                score: r.relevance_score,
            })
            .collect())
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
            r = self.authorize(self.client.post(self.chat_url()).json(&body)).send() => {
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
                    cache_read_input_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens),
                    // No compatible server bills cache *writes* separately;
                    // the ones with a cache populate it as a side effect of an
                    // ordinary request.
                    cache_creation_input_tokens: None,
                    reasoning_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens),
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

/// Accepts the prefix however a person wrote it in a config file.
///
/// `v3`, `/v3`, and `/v3/` all mean the same thing, and an empty value means
/// the routes sit directly on the base URL. Being lenient here costs nothing
/// and saves a class of "why does it 404" that is invisible in a JSON file.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base_url: &str, prefix: Option<&str>) -> OpenAiProvider {
        OpenAiProvider::new("test", base_url, None, OpenAiCapabilities::default())
            .with_api_prefix(prefix)
    }

    /// The headers `authorize` actually puts on a request.
    ///
    /// Built through `reqwest` rather than by reading the struct back, so the
    /// assertion is about the bytes on the wire.
    fn auth_headers(key: Option<&str>, header: Option<&str>) -> reqwest::header::HeaderMap {
        let provider = OpenAiProvider::new(
            "test",
            "http://x",
            key.map(str::to_string),
            OpenAiCapabilities::default(),
        )
        .with_api_key_header(header);

        provider
            .authorize(provider.client.get("http://x"))
            .build()
            .expect("the request must be constructible")
            .headers()
            .clone()
    }

    #[tokio::test]
    async fn a_declared_list_is_the_model_list() {
        // The base URL is deliberately unroutable: if `models()` reached for
        // `/v1/models` at all this would fail rather than answer, which is the
        // whole claim being made — declared models cost no request, so a
        // gateway with no listing route can still offer more than one model.
        let provider = OpenAiProvider::new(
            "apim",
            "http://127.0.0.1:1",
            None,
            OpenAiCapabilities::default(),
        )
        .with_models(vec![
            ModelSpec::new("gpt-4o"),
            ModelSpec {
                id: "llama-3.1-8b".into(),
                display_name: Some("Llama 3.1 8B".into()),
                ..ModelSpec::default()
            },
        ]);

        let models = provider
            .models()
            .await
            .expect("declared models cannot fail");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["gpt-4o", "llama-3.1-8b"]
        );
        // An id is its own label unless the config gave it a better one.
        assert_eq!(models[0].display_name, "gpt-4o");
        assert_eq!(models[1].display_name, "Llama 3.1 8B");
    }

    #[tokio::test]
    async fn a_model_can_override_the_provider_it_is_served_by() {
        // One gateway, two models that share nothing. Told the provider-wide
        // 128k, the 8k model compacts tens of thousands of tokens too late —
        // which is a context-overflow error, not a formatting nicety.
        let provider = OpenAiProvider::new(
            "apim",
            "http://127.0.0.1:1",
            None,
            OpenAiCapabilities {
                native_tools: true,
                vision: true,
                context_length: 128_000,
            },
        )
        .with_models(vec![
            ModelSpec::new("gpt-4o"),
            ModelSpec {
                id: "llama-3.1-8b".into(),
                context_length: Some(8192),
                native_tools: Some(false),
                vision: Some(false),
                ..ModelSpec::default()
            },
        ]);

        let inherited = provider.capabilities("gpt-4o").await.unwrap();
        assert_eq!(inherited.context_length, 128_000);
        assert!(inherited.native_tools);
        assert!(inherited.vision);

        let overridden = provider.capabilities("llama-3.1-8b").await.unwrap();
        assert_eq!(overridden.context_length, 8192);
        assert!(!overridden.native_tools);
        assert!(!overridden.vision);
    }

    #[tokio::test]
    async fn an_openai_model_reads_images_unless_told_otherwise() {
        // The hosted API has not served a text-only chat model since gpt-4o.
        // Defaulting this off refused every screenshot sent to the provider
        // most people configure first, and no config could turn it back on.
        let provider = OpenAiProvider::new(
            "openai",
            "http://127.0.0.1:1",
            None,
            OpenAiCapabilities::default(),
        );

        assert!(provider.capabilities("gpt-5.6-sol").await.unwrap().vision);
    }

    #[tokio::test]
    async fn a_model_nobody_declared_still_gets_the_provider_defaults() {
        // Resuming a conversation started before the list was trimmed, or a
        // model named by `default_model` alone. Neither is a reason to fail.
        let provider = OpenAiProvider::new(
            "apim",
            "http://127.0.0.1:1",
            None,
            OpenAiCapabilities {
                native_tools: false,
                vision: false,
                context_length: 32_000,
            },
        )
        .with_models(vec![ModelSpec::new("gpt-4o")]);

        let caps = provider.capabilities("something-else").await.unwrap();
        assert_eq!(caps.context_length, 32_000);
        assert!(!caps.native_tools);
    }

    #[test]
    fn a_key_with_no_header_named_is_still_a_bearer_token() {
        // The default has to stay byte-identical: every existing config relies
        // on it, and a silent change here reads as "my API key stopped working".
        let headers = auth_headers(Some("sk-abc"), None);
        assert_eq!(headers["authorization"], "Bearer sk-abc");
    }

    #[test]
    fn a_named_header_carries_the_key_with_no_scheme_prefix() {
        // Azure APIM: `Ocp-Apim-Subscription-Key: <key>`, bare. A `Bearer `
        // in front of it is a 401 that looks like a wrong key.
        let headers = auth_headers(Some("sub-key"), Some("Ocp-Apim-Subscription-Key"));
        assert_eq!(headers["ocp-apim-subscription-key"], "sub-key");
        assert!(
            !headers.contains_key("authorization"),
            "the bearer header must not also be sent"
        );
    }

    #[test]
    fn azure_openais_own_header_works_the_same_way() {
        let headers = auth_headers(Some("azure-key"), Some("api-key"));
        assert_eq!(headers["api-key"], "azure-key");
    }

    #[test]
    fn naming_authorization_sends_the_key_without_bearer() {
        // The reason there is no separate prefix setting: a gateway wanting a
        // bare Authorization is expressible with the one knob.
        let headers = auth_headers(Some("raw-token"), Some("Authorization"));
        assert_eq!(headers["authorization"], "raw-token");
    }

    #[test]
    fn the_key_is_marked_sensitive_so_it_stays_out_of_debug_output() {
        // reqwest renders headers with `{:?}` when tracing a request.
        let headers = auth_headers(Some("sub-key"), Some("Ocp-Apim-Subscription-Key"));
        assert!(headers["ocp-apim-subscription-key"].is_sensitive());
        assert!(!format!("{headers:?}").contains("sub-key"));
    }

    #[test]
    fn a_header_name_is_accepted_however_it_was_written() {
        for written in [" api-key ", "api-key"] {
            assert_eq!(auth_headers(Some("k"), Some(written))["api-key"], "k");
        }
        // Blank means "not set", not "a header with no name", which would be
        // an unbuildable request rather than a config error the user can see.
        for written in ["", "   "] {
            assert_eq!(
                auth_headers(Some("k"), Some(written))["authorization"],
                "Bearer k",
                "{written:?} should fall back to bearer auth"
            );
        }
    }

    #[test]
    fn no_key_configured_sends_no_credential_either_way() {
        assert!(auth_headers(None, None).get("authorization").is_none());
        let named = auth_headers(None, Some("api-key"));
        assert!(named.get("api-key").is_none());
    }

    #[test]
    fn a_key_that_cannot_be_a_header_value_is_dropped_rather_than_panicking() {
        // A pasted key with a trailing newline in the env var. Sending nothing
        // yields a 401 the user can act on; unwrapping would take down the app.
        let headers = auth_headers(Some("bad\nkey"), Some("api-key"));
        assert!(headers.get("api-key").is_none());
    }

    #[test]
    fn the_default_prefix_is_what_almost_every_server_uses() {
        let p = provider("http://localhost:8000", None);
        assert_eq!(
            p.chat_url(),
            "http://localhost:8000/v1/chat/completions",
            "the default must stay byte-identical to the old hardcoded route"
        );
        assert_eq!(p.models_url(), "http://localhost:8000/v1/models");
    }

    #[test]
    fn an_openvino_model_server_can_be_moved_to_its_own_prefix() {
        // OVMS before 2026.3 serves the OpenAI routes under /v3 only.
        let p = provider("http://localhost:8000", Some("/v3"));
        assert_eq!(p.chat_url(), "http://localhost:8000/v3/chat/completions");
        assert_eq!(p.models_url(), "http://localhost:8000/v3/models");
    }

    #[test]
    fn the_prefix_is_never_applied_twice() {
        // Regression: the routes were once written as `url("/v1/models")`,
        // which silently became `/v1/v1/models` once a prefix existed.
        for prefix in [None, Some("/v1"), Some("/v3")] {
            let p = provider("http://x", prefix);
            assert_eq!(
                p.models_url().matches("/v1/").count() + p.models_url().matches("/v3/").count(),
                1
            );
            assert!(!p.chat_url().contains("/v1/v1/"), "{}", p.chat_url());
        }
    }

    #[test]
    fn the_prefix_is_accepted_however_it_was_written() {
        for written in ["/v3", "v3", "/v3/", " v3 "] {
            assert_eq!(
                provider("http://x", Some(written)).models_url(),
                "http://x/v3/models",
                "{written:?} should mean the same thing"
            );
        }
    }

    #[test]
    fn the_rerank_route_follows_the_configured_prefix() {
        // Same trap the other two routes already have a test for: written
        // inline as `url("/v1/rerank")` this becomes `/v1/v1/rerank` on a
        // default config and nothing catches it.
        assert_eq!(
            provider("http://x", None).rerank_url(),
            "http://x/v1/rerank"
        );
        assert_eq!(
            provider("http://x", Some("/v3")).rerank_url(),
            "http://x/v3/rerank"
        );
    }

    #[test]
    fn a_rerank_body_asks_for_ranks_rather_than_the_documents_back() {
        let documents = vec!["alpha".to_string(), "beta".to_string()];
        let body = serde_json::to_value(RerankBody {
            model: "bge-reranker-v2-m3",
            query: "where the retry backoff is",
            documents: &documents,
            top_n: documents.len(),
            return_documents: false,
        })
        .expect("serializes");

        assert_eq!(body["top_n"], 2);
        assert_eq!(
            body["return_documents"], false,
            "the caller still holds the documents; echoing them back doubles \
             the response for nothing"
        );
        assert_eq!(body["documents"][1], "beta");
    }

    #[test]
    fn a_reranked_response_is_read_whichever_name_the_server_gives_the_score() {
        // Cohere, Jina, Voyage and llama.cpp say `relevance_score`; TEI says
        // `score`. Both are the same field and neither is wrong.
        for field in ["relevance_score", "score"] {
            let raw = format!(r#"{{"results":[{{"index":1,"{field}":-4.75}}]}}"#);
            let parsed: RerankResponse = serde_json::from_str(&raw).expect("{field} should parse");
            assert_eq!(parsed.results[0].index, 1);
            assert_eq!(parsed.results[0].relevance_score, -4.75);
        }
    }

    #[tokio::test]
    async fn reranking_nothing_asks_the_server_nothing() {
        // The empty shortlist is reachable: a search that matched no passages
        // still runs the tool to the end. A round trip to score zero documents
        // is a round trip that can fail, so it is not made.
        let scores = provider("http://127.0.0.1:1", None)
            .rerank("any-model", "any query", &[])
            .await
            .expect("an empty rerank must not touch the network");
        assert!(scores.is_empty());
    }

    #[tokio::test]
    async fn a_backend_with_no_reranking_route_says_so_by_name() {
        // The default on the trait, exercised through a provider that never
        // implemented it. The message has to name the provider, because the
        // user's next move is to point the setting at a different one.
        struct Chatty;

        #[async_trait]
        impl Provider for Chatty {
            fn id(&self) -> &str {
                "ollama"
            }
            async fn models(&self) -> Result<Vec<ModelInfo>> {
                Ok(Vec::new())
            }
            async fn capabilities(&self, _: &str) -> Result<Capabilities> {
                Ok(Capabilities::default())
            }
            async fn stream(
                &self,
                _: ChatRequest,
                _: mpsc::Sender<StreamEvent>,
                _: CancellationToken,
            ) -> Result<StopReason> {
                Ok(StopReason::EndTurn)
            }
        }

        let error = Chatty
            .rerank("m", "q", &["doc".to_string()])
            .await
            .expect_err("the default refuses");
        let message = error.to_string();
        assert!(message.contains("ollama"), "{message}");
        assert!(message.contains("--reranking"), "{message}");
    }

    #[test]
    fn an_empty_prefix_puts_the_routes_on_the_base_url() {
        // What a reverse proxy that already strips the version segment needs.
        for written in ["", "/", "  "] {
            assert_eq!(
                provider("http://x", Some(written)).models_url(),
                "http://x/models",
                "{written:?} should mean no prefix"
            );
        }
    }

    #[test]
    fn a_prefix_may_have_more_than_one_segment() {
        let p = provider("https://gateway.example", Some("/openai/v1"));
        assert_eq!(
            p.chat_url(),
            "https://gateway.example/openai/v1/chat/completions"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let p = provider("http://localhost:8000/", Some("/v3"));
        assert_eq!(p.models_url(), "http://localhost:8000/v3/models");
    }

    #[test]
    fn the_embeddings_route_follows_the_configured_prefix() {
        assert_eq!(
            provider("http://x", None).embeddings_url(),
            "http://x/v1/embeddings"
        );
        assert_eq!(
            provider("http://x", Some("/v3")).embeddings_url(),
            "http://x/v3/embeddings"
        );
    }

    #[test]
    fn an_embedding_body_sends_an_array_even_for_one_string() {
        // The API takes a bare string too. Always sending the array means one
        // code path rather than two that have to agree about the response.
        let inputs = vec!["one chunk".to_string()];
        let body = serde_json::to_value(EmbedBody {
            model: "text-embedding-3-small",
            input: &inputs,
        })
        .expect("serializes");
        assert!(body["input"].is_array());
        assert_eq!(body["input"][0], "one chunk");
    }

    #[tokio::test]
    async fn embedding_nothing_asks_the_server_nothing() {
        let vectors = provider("http://127.0.0.1:1", None)
            .embed("any-model", &[])
            .await
            .expect("an empty batch must not touch the network");
        assert!(vectors.is_empty());
    }

    #[test]
    fn embeddings_are_placed_by_the_index_the_server_reported() {
        // Out of order on the wire, in order in the result. A vector attached
        // to the wrong chunk is a search that confidently returns the wrong
        // file, which is far harder to notice than one that fails.
        let raw = r#"{"data":[
            {"index":2,"embedding":[3.0]},
            {"index":0,"embedding":[1.0]},
            {"index":1,"embedding":[2.0]}
        ]}"#;
        let parsed: EmbedResponse = serde_json::from_str(raw).expect("parses");
        let mut placed = vec![None; 3];
        for item in parsed.data {
            placed[item.index] = Some(item.embedding);
        }
        assert_eq!(
            placed,
            vec![Some(vec![1.0]), Some(vec![2.0]), Some(vec![3.0])]
        );
    }
}
