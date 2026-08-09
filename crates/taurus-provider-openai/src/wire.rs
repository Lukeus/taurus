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
}
