//! Provider-agnostic types for the Taurus agent harness.
//!
//! Nothing in this crate knows about a specific backend. Adapters
//! (`taurus-provider-ollama`, `taurus-provider-openai`) implement [`Provider`]
//! and translate to and from these types; `taurus-core` drives the agent loop
//! against the trait alone.

pub mod error;
pub mod message;
pub mod prompted;
pub mod provider;
pub mod request;
pub mod stream;

pub use error::{ProviderError, Result};
pub use message::{ContentBlock, Message, Role};
pub use prompted::PromptedTools;
pub use provider::{Capabilities, ModelInfo, Provider, RerankScore};
pub use request::{ChatRequest, ToolDef};
pub use stream::{StopReason, StreamAccumulator, StreamEvent, TokenUsage};

/// Fresh tool-call id. Used by adapters whose wire format omits one.
pub fn new_tool_use_id() -> String {
    format!("tu_{}", uuid::Uuid::new_v4().simple())
}
