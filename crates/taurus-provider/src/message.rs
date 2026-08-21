//! Normalized conversation types.
//!
//! These deliberately follow the Anthropic content-block shape rather than the
//! OpenAI `tool_calls` + `role: "tool"` shape. Content blocks are the superset:
//! an OpenAI or Ollama response maps into them without loss, while the reverse
//! direction cannot represent interleaved text, thinking, and multiple tool
//! calls inside a single assistant turn.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Chain-of-thought emitted by reasoning models. Kept separate from `Text`
    /// so the UI can collapse it and compaction can drop it first.
    Thinking {
        text: String,
        /// Opaque proof-of-origin, for providers that issue one and require it
        /// back verbatim.
        ///
        /// Anthropic is the case: a turn that thought and then called a tool is
        /// only legal on the next request if its thinking blocks come back
        /// signed and unedited, and dropping them is a 400 rather than a
        /// degradation. Carried rather than regenerated because it is a
        /// signature — the whole point is that this harness cannot produce one.
        ///
        /// `None` on every other backend, and on transcripts written before
        /// this field existed, which is why it defaults rather than being
        /// required.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        #[ts(type = "unknown")]
        input: serde_json::Value,
        /// Opaque proof-of-origin for the *call*, for providers that issue one.
        ///
        /// Gemini is the case, and it is separate from the one on `Thinking`:
        /// a model that reasoned its way to a tool call signs the call itself,
        /// and replaying that call without the signature is a 400 naming the
        /// tool and its position in the history. Anthropic signs the thinking
        /// instead, which is why this is a second field rather than the same
        /// one moved.
        ///
        /// `None` on every other backend, and on transcripts written before
        /// this field existed, which is why it defaults rather than being
        /// required.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        signature: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    Image {
        mime_type: String,
        /// Base64, no data-URI prefix.
        data: String,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Reasoning with no signature, which is every provider but Anthropic.
    pub fn thinking(text: impl Into<String>) -> Self {
        Self::Thinking {
            text: text.into(),
            signature: None,
        }
    }

    /// A call with no signature, which is every provider but Gemini.
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
            signature: None,
        }
    }

    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    pub fn tool_error(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: true,
        }
    }

    /// Text carried by this block, if any. Tool results count: compaction and
    /// token estimation care about their size.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } | Self::Thinking { text, .. } => Some(text),
            Self::ToolResult { content, .. } => Some(content),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::text(text)])
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![ContentBlock::text(text)])
    }

    /// Concatenated plain text of the message, ignoring thinking and tool blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &serde_json::Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }

    pub fn has_tool_use(&self) -> bool {
        self.tool_uses().next().is_some()
    }
}
