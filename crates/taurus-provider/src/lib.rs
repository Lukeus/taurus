//! Provider-agnostic types for the Taurus agent harness.
//!
//! Nothing in this crate knows about a specific backend. Adapters
//! (`taurus-provider-ollama`, `taurus-provider-openai`) implement [`Provider`]
//! and translate to and from these types; `taurus-core` drives the agent loop
//! against the trait alone.

pub mod error;
#[cfg(feature = "http")]
pub mod http;
pub mod image;
pub mod message;
pub mod prompted;
pub mod provider;
pub mod request;
pub mod stream;

pub use error::{brief, ProviderError, Result};
pub use message::{
    anthropic_signature, gemini_signature, is_openai_reasoning, relocated_note, replay_input,
    with_unique_tool_ids, ContentBlock, Message, Role, ToolOutput, ToolResultBlock,
    GEMINI_THOUGHT_PREFIX, OPENAI_REASONING_PREFIX,
};
pub use prompted::PromptedTools;
pub use provider::{Capabilities, ModelInfo, Provider, RerankScore};
pub use request::{ChatRequest, ToolDef};
pub use stream::{ModelSpend, Spend, StopReason, StreamAccumulator, StreamEvent, TokenUsage};

/// Fresh tool-call id. Used by adapters whose wire format omits one.
pub fn new_tool_use_id() -> String {
    format!("tu_{}", uuid::Uuid::new_v4().simple())
}

/// Reads an explicit `null` as the type's default.
///
/// `#[serde(default)]` only covers a field that's absent. A server built on a
/// model class writes every unset field as `null`, and on a `Vec` or a `String`
/// that fails the whole frame, which the adapters skip with a warning: an entire
/// answer can vanish that way, one chunk at a time, with no error. Goes with
/// `default`: `#[serde(default, deserialize_with = "taurus_provider::null_as_default")]`.
pub fn null_as_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(<Option<T> as serde::Deserialize>::deserialize(deserializer)?.unwrap_or_default())
}
