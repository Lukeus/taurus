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

/// One document's relevance to a query, as a reranking model scored it.
///
/// Carries the index rather than the text: the caller already holds the
/// documents in order, and sending them back would double the size of a
/// response whose only job is to say which order they belong in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RerankScore {
    /// Position in the `documents` slice this was scored from.
    pub index: usize,
    /// How relevant this document is to the query — **higher is more relevant,
    /// and that is the only guarantee.**
    ///
    /// Deliberately not "between 0 and 1", because the backends disagree.
    /// Voyage and Cohere normalize; llama.cpp returns the cross-encoder's raw
    /// logit, where negative values are ordinary and the spread between a good
    /// match and a bad one is several whole numbers rather than a fraction.
    ///
    /// So this orders documents and does nothing else. Code that thresholded
    /// it at 0.5, multiplied two of them together, or compared one response's
    /// scores against another's would silently discard correct results on
    /// whichever backend it was not written against — and silently is the
    /// problem, because the answer still looks like an answer.
    pub score: f32,
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

    /// Scores `documents` by how well each answers `query`, best-first order
    /// left to the caller to apply.
    ///
    /// A second retrieval stage, not a replacement for the first. Embeddings
    /// score a query and a passage separately and compare the results, which is
    /// what makes an index searchable at all — every vector can be computed
    /// once and kept. A reranker reads the query and the passage *together* and
    /// is much better at it, which is why it cannot be an index: the work is
    /// per pair, so it only makes sense over a shortlist something cheaper
    /// already drew up.
    ///
    /// That division is the whole reason this is worth having here. The
    /// constraint everything in this harness is shaped around is a context
    /// window that cannot afford three wrong `read_file` calls, and the way to
    /// spend fewer of them is not to retrieve more — it is to be right about
    /// the five passages retrieved.
    ///
    /// On [`Provider`] for the same reason [`Provider::embed`] is: it is a
    /// thing a *backend* does or does not do, and a separate trait would mean a
    /// second registry and a second configuration entry naming the same server,
    /// with no way to stop the two from disagreeing about which machine to talk
    /// to.
    ///
    /// The default refuses. Most backends have no reranking route — Ollama has
    /// none as of this writing — and the fix is to name one that does, so the
    /// message says which provider was asked rather than a bare "unsupported".
    ///
    /// `model` is a reranking model. It is a third namespace, separate from
    /// both the chat models and the embedding models, on every backend that
    /// serves more than one of the three.
    ///
    /// Implementations must return at most one score per document and never an
    /// index outside `documents`. Returning *fewer* is allowed — a server
    /// honoring its own `top_n` does exactly that — and callers must treat an
    /// unscored document as ranked below every scored one rather than dropping
    /// it.
    async fn rerank(
        &self,
        model: &str,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<RerankScore>> {
        let _ = (model, query, documents);
        Err(crate::error::ProviderError::Protocol(format!(
            "{} does not rerank. Point reranking at a backend that does — an OpenAI-compatible \
             server with a reranking model loaded, such as llama.cpp started with `--reranking`.",
            self.id()
        )))
    }
}
