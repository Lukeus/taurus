//! OpenAI chat-completions wire types.

use serde::{Deserialize, Serialize};
use taurus_provider::ChatRequest;

#[derive(Debug, Serialize)]
pub struct ChatBody {
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    /// Ask for a final usage frame; servers that do not know this option
    /// ignore it, and we simply report zeros.
    pub stream_options: serde_json::Value,
}

impl ChatBody {
    pub fn from_request(request: &ChatRequest) -> Self {
        Self {
            model: request.model.clone(),
            messages: crate::convert::messages_to_wire(request),
            stream: true,
            tools: crate::convert::tools_to_wire(&request.tools),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stop: request.stop_sequences.clone(),
            stream_options: serde_json::json!({ "include_usage": true }),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    #[serde(default)]
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub delta: Option<Delta>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    /// Non-standard but widely used by reasoning-model servers.
    #[serde(default, alias = "reasoning")]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallDelta {
    /// Position in the call list. Continuation frames carry only this.
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    /// A *string* fragment of JSON, not an object. Concatenates across frames.
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    /// Where the cache hit is reported. Absent on most compatible servers,
    /// which have no cache; present on OpenAI itself and on the gateways that
    /// imitate it closely enough to bill for one.
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Deserialize)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct CompletionTokensDetails {
    /// Counted inside `completion_tokens` rather than beside it, so adding the
    /// two would bill the reasoning twice.
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}

/// A reranking request.
///
/// The Cohere-shaped body, which is what every server serving this route
/// speaks — llama.cpp, text-embeddings-inference, Jina, Voyage, Cohere itself.
/// There is no OpenAI original to be compatible with here: OpenAI has never
/// shipped a reranking endpoint, and the imitators standardized on Cohere's
/// instead.
#[derive(Debug, Serialize)]
pub struct RerankBody<'a> {
    pub model: &'a str,
    pub query: &'a str,
    pub documents: &'a [String],
    /// Asked for explicitly rather than left to the server's default, which is
    /// not the same number everywhere and on some servers is "all of them".
    pub top_n: usize,
    /// The documents come back only if asked for, and they are not wanted: the
    /// caller sent them and still holds them in order. Sent as `false` rather
    /// than omitted because at least one server defaults it on.
    pub return_documents: bool,
}

#[derive(Debug, Deserialize)]
pub struct RerankResponse {
    #[serde(default)]
    pub results: Vec<RerankResult>,
}

#[derive(Debug, Deserialize)]
pub struct RerankResult {
    pub index: usize,
    /// Cohere, Jina, Voyage and llama.cpp all spell it this way. TEI answers
    /// with `score` instead, which is why the alias is here rather than a
    /// second response type.
    #[serde(alias = "score")]
    pub relevance_score: f32,
}

/// An embedding request.
///
/// `input` is an array even for one string. The API accepts a bare string too
/// and answers the same shape either way, but sending the array unconditionally
/// means one code path rather than two that have to agree.
#[derive(Debug, Serialize)]
pub struct EmbedBody<'a> {
    pub model: &'a str,
    pub input: &'a [String],
}

#[derive(Debug, Deserialize)]
pub struct EmbedResponse {
    #[serde(default)]
    pub data: Vec<Embedding>,
}

#[derive(Debug, Deserialize)]
pub struct Embedding {
    /// Which input this answers. Documented as returned in order, and not
    /// trusted to be: a vector attached to the wrong chunk is a search that
    /// quietly returns the wrong file, which is worse than one that fails.
    #[serde(default)]
    pub index: usize,
    pub embedding: Vec<f32>,
}
