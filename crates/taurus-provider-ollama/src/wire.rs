//! Ollama wire types.
//!
//! Ollama streams newline-delimited JSON, not SSE, and delivers tool call
//! arguments as a JSON *object* rather than OpenAI's stringified JSON. Both
//! differences are absorbed here so nothing above this module knows about them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct ChatBody {
    pub model: String,
    pub messages: Vec<WireMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Options>,
}

#[derive(Debug, Default, Serialize)]
pub struct Options {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

impl Options {
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none() && self.num_predict.is_none() && self.stop.is_empty()
    }
}

#[derive(Debug, Serialize)]
pub struct WireMessage {
    pub role: &'static str,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
    /// Present on `role: "tool"` messages so the model can match a result to
    /// the call that produced it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunction,
}

#[derive(Debug, Serialize)]
pub struct WireFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub function: WireToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireToolCallFunction {
    pub name: String,
    /// An object, not a string. Ollama differs from OpenAI here.
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// One line of the `/api/chat` stream.
#[derive(Debug, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub message: Option<ChunkMessage>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    #[serde(default)]
    pub prompt_eval_count: Option<u32>,
    #[serde(default)]
    pub eval_count: Option<u32>,
    /// Present instead of `message` when the server rejects the request
    /// mid-stream (unknown model, bad options).
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChunkMessage {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<WireToolCall>,
}

#[derive(Debug, Deserialize)]
pub struct TagsResponse {
    #[serde(default)]
    pub models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
pub struct TagModel {
    pub name: String,
    #[serde(default)]
    pub details: Option<TagDetails>,
}

#[derive(Debug, Deserialize)]
pub struct TagDetails {
    #[serde(default)]
    pub parameter_size: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShowResponse {
    /// e.g. `["completion", "vision", "tools", "thinking"]`
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub model_info: serde_json::Map<String, serde_json::Value>,
}

impl ShowResponse {
    /// The context length key is namespaced by architecture (`gemma3.context_length`,
    /// `qwen35.context_length`, ...), so match on the suffix rather than
    /// maintaining a table of architecture names.
    pub fn context_length(&self) -> Option<u32> {
        self.model_info
            .iter()
            .find(|(k, _)| k.ends_with(".context_length"))
            .and_then(|(_, v)| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
    }
}
