//! The trait every model backend implements.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::error::Result;
use crate::request::ChatRequest;
use crate::stream::{StopReason, StreamEvent};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    /// None when the backend does not report it; callers fall back to
    /// `Capabilities::context_length`.
    pub context_length: Option<u32>,
}

/// What a specific model on a specific backend can do.
///
/// Resolved per model, not per provider: on Ollama, `qwen3.6:27b` supports
/// native tool calls and `gemma3` does not, from the same server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Capabilities {
    /// False means the harness must fall back to prompted tool calling.
    pub native_tools: bool,
    pub vision: bool,
    /// Model emits separable reasoning content.
    pub thinking: bool,
    pub context_length: u32,
}

impl Default for Capabilities {
    fn default() -> Self {
        // Conservative: assume no native tools so an unknown model gets the
        // prompted fallback (which works everywhere) instead of silently
        // dropping every tool call.
        Self {
            native_tools: false,
            vision: false,
            thinking: false,
            context_length: 8192,
        }
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable identifier used in config and UI, e.g. `"ollama"`.
    fn id(&self) -> &str;

    async fn models(&self) -> Result<Vec<ModelInfo>>;

    async fn capabilities(&self, model: &str) -> Result<Capabilities>;

    /// Streams a single assistant turn.
    ///
    /// Implementations must drop `tx` on return, must stop promptly when
    /// `cancel` fires (returning `StopReason::Canceled`, not an error), and
    /// must not emit events after returning.
    async fn stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> Result<StopReason>;

    /// Turns text into vectors, one per input, in the order given.
    ///
    /// A method on `Provider` rather than a trait of its own because embedding
    /// is a thing a *backend* does or does not do, and every caller that wants
    /// one is already holding a provider. A second trait would mean a second
    /// registry, a second configuration entry naming the same server, and a
    /// way for the two to disagree about which machine to talk to.
    ///
    /// The default refuses, which is right for every backend that has no
    /// embedding endpoint. It names the provider so the message is actionable
    /// rather than a bare "unsupported": the fix is to point the index at a
    /// backend that can, and the user has to know which one this was.
    ///
    /// `model` is an embedding model, not a chat model, and the two namespaces
    /// are separate on every backend that has both.
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let _ = (model, inputs);
        Err(crate::error::ProviderError::Protocol(format!(
            "{} does not produce embeddings. Point the index at a backend that does — a local \
             Ollama with an embedding model pulled is the usual answer.",
            self.id()
        )))
    }
}
