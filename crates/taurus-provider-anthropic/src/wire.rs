//! Anthropic Messages API wire types.

use serde::{Deserialize, Serialize};
use taurus_provider::ChatRequest;

/// Output ceiling when the caller names none.
///
/// `max_tokens` is required by this API, unlike every other backend here, so a
/// default is not a convenience but a necessity. Sized for a turn that thinks
/// before it answers: the ceiling covers reasoning *and* the reply, so a value
/// tuned to the reply alone truncates mid-answer on a model whose thinking is
/// on by default.
pub const DEFAULT_MAX_TOKENS: u32 = 32_000;

#[derive(Debug, Serialize)]
pub struct MessagesBody {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<serde_json::Value>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Omitted unless the config asks for a specific mode.
    ///
    /// Omitting is the only value valid on every model this API has served:
    /// newer ones think by default and older ones do not, and either way the
    /// request is accepted. Naming a mode is how a caller overrides that, and
    /// the reason it is not named here is that the wrong one is a 400 rather
    /// than a preference — `budget_tokens` is rejected outright on current
    /// models, and disabling thinking is rejected above a certain effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
}

impl MessagesBody {
    pub fn from_request(request: &ChatRequest, thinking: Option<serde_json::Value>) -> Self {
        Self {
            model: request.model.clone(),
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            messages: crate::convert::messages_to_wire(request),
            stream: true,
            system: crate::convert::system_to_wire(request.system.as_deref()),
            tools: crate::convert::tools_to_wire(&request.tools),
            temperature: request.temperature,
            stop_sequences: request.stop_sequences.clone(),
            thinking,
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
    #[serde(default)]
    pub display_name: Option<String>,
    /// The context window. Named for the direction it bounds, and distinct from
    /// `max_tokens`, which is the output cap — reading the wrong one gives a
    /// budget sixteen times too small.
    #[serde(default)]
    pub max_input_tokens: Option<u32>,
    #[serde(default)]
    pub capabilities: Option<serde_json::Value>,
}

impl ModelEntry {
    /// Whether the model takes images, as the models endpoint reports it.
    ///
    /// The capability tree is read by path rather than typed out: it grows a
    /// branch whenever a feature ships, and a struct that has to be updated for
    /// each one would fail to parse a model newer than this binary.
    pub fn vision(&self) -> Option<bool> {
        self.capability(&["image_input", "supported"])
    }

    pub fn thinking(&self) -> Option<bool> {
        self.capability(&["thinking", "supported"])
    }

    fn capability(&self, path: &[&str]) -> Option<bool> {
        let mut node = self.capabilities.as_ref()?;
        for key in path {
            node = node.get(key)?;
        }
        node.as_bool()
    }
}

/// One frame of the event stream.
///
/// Every variant this adapter acts on, and `#[serde(other)]` for the rest:
/// `ping` carries nothing, and a frame type added after this binary was built
/// must not stop the stream.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamFrame {
    MessageStart {
        message: MessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: BlockStart,
    },
    ContentBlockDelta {
        index: u32,
        delta: BlockDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaBody,
        #[serde(default)]
        usage: Option<Usage>,
    },
    MessageStop,
    /// Arrives with HTTP 200 already sent, so it cannot surface as a status code.
    Error {
        error: ApiError,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct MessageStart {
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockStart {
    Text,
    Thinking,
    /// Reasoning the provider withheld. It has no readable text and no
    /// signature this harness can replay, so it opens a block and contributes
    /// nothing to it.
    RedactedThinking,
    ToolUse {
        id: String,
        name: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    /// The proof of origin for the open thinking block, arriving in fragments
    /// like tool input does.
    SignatureDelta {
        signature: String,
    },
    /// A *string* fragment of the tool input, concatenated across frames.
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct MessageDeltaBody {
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: Option<u32>,
    #[serde(default)]
    pub output_tokens: Option<u32>,
    /// Tokens served from the prompt cache, billed at about a tenth of a fresh
    /// read. Counted into the input total so `taurus usage` reports what the
    /// request actually carried rather than only the part that missed.
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
}

impl Usage {
    pub fn input_total(&self) -> u32 {
        self.input_tokens
            .unwrap_or(0)
            .saturating_add(self.cache_read_input_tokens.unwrap_or(0))
            .saturating_add(self.cache_creation_input_tokens.unwrap_or(0))
    }
}

#[derive(Debug, Deserialize)]
pub struct ApiError {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_frame_type_does_not_stop_the_stream() {
        // `ping` today, and whatever ships next. A stream that dies on an
        // unrecognized frame breaks on an API change this adapter did not need
        // to care about.
        let frame: StreamFrame = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(frame, StreamFrame::Other));
        let frame: StreamFrame = serde_json::from_str(r#"{"type":"something_new","x":1}"#).unwrap();
        assert!(matches!(frame, StreamFrame::Other));
    }

    #[test]
    fn cached_input_counts_toward_the_input_total() {
        // Otherwise a well-cached turn reports a handful of input tokens and
        // the usage report understates what the request carried.
        let usage = Usage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_read_input_tokens: Some(4000),
            cache_creation_input_tokens: Some(50),
        };
        assert_eq!(usage.input_total(), 4150);
    }

    #[test]
    fn a_models_entry_reads_the_context_window_from_max_input_tokens() {
        // `max_tokens` on the same object is the *output* cap. Reading it as
        // the window gives a budget sixteen times too small.
        let entry: ModelEntry = serde_json::from_str(
            r#"{"id":"claude-opus-5","display_name":"Claude Opus 5",
                "max_input_tokens":1000000,"max_tokens":128000,
                "capabilities":{"image_input":{"supported":true}}}"#,
        )
        .unwrap();
        assert_eq!(entry.max_input_tokens, Some(1_000_000));
        assert_eq!(entry.vision(), Some(true));
        assert_eq!(entry.thinking(), None);
    }

    #[test]
    fn a_capability_tree_this_binary_does_not_know_is_not_an_error() {
        let entry: ModelEntry =
            serde_json::from_str(r#"{"id":"m","capabilities":{"brand_new":{"supported":true}}}"#)
                .unwrap();
        assert_eq!(entry.vision(), None);
    }

    #[test]
    fn thinking_is_omitted_from_the_body_unless_asked_for() {
        // The only value valid on every model this API has served.
        let request = ChatRequest::new("claude-opus-5", vec![taurus_provider::Message::user("hi")]);
        let body = MessagesBody::from_request(&request, None);
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("thinking").is_none(), "{json}");
        assert_eq!(json["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn a_caller_supplied_ceiling_wins_over_the_default() {
        let mut request =
            ChatRequest::new("claude-opus-5", vec![taurus_provider::Message::user("hi")]);
        request.max_tokens = Some(1024);
        let body = MessagesBody::from_request(&request, None);
        assert_eq!(body.max_tokens, 1024);
    }
}
