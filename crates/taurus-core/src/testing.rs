//! A scripted provider for exercising the loop without a model.
//!
//! Every behavior the agent loop is responsible for — tool dispatch, ordering,
//! error recovery, cancellation, the iteration ceiling — is deterministic given
//! the model's output, so the tests script that output directly instead of
//! hoping a real model cooperates.

use std::sync::Arc;

use async_trait::async_trait;
use taurus_provider::{
    Capabilities, ChatRequest, ModelInfo, Provider, ProviderError, Result, StopReason, StreamEvent,
};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

/// One scripted assistant turn.
pub struct ScriptedTurn {
    pub events: Vec<StreamEvent>,
    pub stop: StopReason,
    /// When set, the request fails with this instead of completing.
    ///
    /// `events` are still emitted first, so a failure part-way through a stream
    /// is scriptable too — which is the case the loop must *not* retry, and so
    /// the one worth being able to write a test for.
    pub failure: Option<ProviderError>,
}

impl ScriptedTurn {
    /// A turn that just says something and ends.
    pub fn text(text: &str) -> Self {
        Self {
            events: vec![StreamEvent::TextDelta { text: text.into() }],
            stop: StopReason::EndTurn,
            failure: None,
        }
    }

    /// A turn streamed a token at a time, the way a real one arrives.
    ///
    /// `ScriptedTurn::text` sends the whole answer as one delta, which is the
    /// shape almost every test wants and the one shape that cannot show what
    /// the loop does with a *run* of them.
    pub fn tokens(tokens: &[&str]) -> Self {
        Self {
            events: tokens
                .iter()
                .map(|text| StreamEvent::TextDelta {
                    text: (*text).into(),
                })
                .collect(),
            stop: StopReason::EndTurn,
            failure: None,
        }
    }

    /// A turn that thinks out loud before answering.
    pub fn thinks_then_says(thinking: &[&str], text: &[&str]) -> Self {
        let mut events: Vec<StreamEvent> = thinking
            .iter()
            .map(|t| StreamEvent::ThinkingDelta { text: (*t).into() })
            .collect();
        events.extend(
            text.iter()
                .map(|t| StreamEvent::TextDelta { text: (*t).into() }),
        );
        Self {
            events,
            stop: StopReason::EndTurn,
            failure: None,
        }
    }

    /// A request that fails before producing anything, with a status the
    /// provider layer classifies as worth retrying.
    pub fn transient_failure() -> Self {
        Self {
            events: Vec::new(),
            stop: StopReason::EndTurn,
            failure: Some(ProviderError::Api {
                provider: "fake".into(),
                status: 503,
                body: "upstream is briefly unavailable".into(),
            }),
        }
    }

    /// A request that streams some text and *then* fails. Retrying this would
    /// replay text the user has already read.
    pub fn transient_failure_after_text(text: &str) -> Self {
        Self {
            events: vec![StreamEvent::TextDelta { text: text.into() }],
            stop: StopReason::EndTurn,
            failure: Some(ProviderError::Api {
                provider: "fake".into(),
                status: 503,
                body: "died mid-answer".into(),
            }),
        }
    }

    /// A request that fails in a way no retry can fix.
    pub fn permanent_failure() -> Self {
        Self {
            events: Vec::new(),
            stop: StopReason::EndTurn,
            failure: Some(ProviderError::Api {
                provider: "fake".into(),
                status: 401,
                body: "invalid api key".into(),
            }),
        }
    }

    /// A turn that calls one tool.
    pub fn tool_call(id: &str, name: &str, input: serde_json::Value) -> Self {
        Self {
            events: vec![
                StreamEvent::ToolUseStart {
                    id: id.into(),
                    name: name.into(),
                },
                StreamEvent::ToolUseInputDelta {
                    id: id.into(),
                    json: input.to_string(),
                },
                StreamEvent::ToolUseEnd { id: id.into() },
            ],
            stop: StopReason::ToolUse,
            failure: None,
        }
    }

    /// A turn that calls several tools at once.
    pub fn tool_calls(calls: Vec<(&str, &str, serde_json::Value)>) -> Self {
        let mut events = Vec::new();
        for (id, name, input) in calls {
            events.push(StreamEvent::ToolUseStart {
                id: id.into(),
                name: name.into(),
            });
            events.push(StreamEvent::ToolUseInputDelta {
                id: id.into(),
                json: input.to_string(),
            });
            events.push(StreamEvent::ToolUseEnd { id: id.into() });
        }
        Self {
            events,
            stop: StopReason::ToolUse,
            failure: None,
        }
    }
}

pub struct FakeProvider {
    turns: Mutex<std::collections::VecDeque<ScriptedTurn>>,
    /// Requests the loop actually sent, for asserting on prompt construction.
    pub seen: Mutex<Vec<ChatRequest>>,
    context_length: u32,
    /// Turn used once the script runs out. Without it, a loop bug would hang
    /// the test rather than fail it.
    fallback_text: String,
    /// Fires the cancellation token when this many requests have been served.
    ///
    /// Cancellation is otherwise untestable here: a scripted provider answers
    /// in microseconds, so a timer race would decide the result rather than
    /// the loop's behavior.
    cancel_after_requests: Option<usize>,
    /// Whether this backend reads images.
    ///
    /// False by default, because that is the interesting side: it is what makes
    /// the loop strip pictures out of the history, and a fake that could always
    /// see would let that path go untested in every test but the one written
    /// for it.
    vision: bool,
}

impl FakeProvider {
    pub fn new(turns: Vec<ScriptedTurn>) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into()),
            seen: Mutex::new(Vec::new()),
            context_length: 128_000,
            fallback_text: "(script exhausted)".into(),
            cancel_after_requests: None,
            vision: false,
        })
    }

    /// The same, on a backend that reads images.
    pub fn seeing(turns: Vec<ScriptedTurn>) -> Arc<Self> {
        let mut provider = Arc::into_inner(Self::new(turns)).expect("just built");
        provider.vision = true;
        Arc::new(provider)
    }

    /// A provider that cancels the turn once it has served `n` requests.
    pub fn cancelling_after(turns: Vec<ScriptedTurn>, n: usize) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into()),
            seen: Mutex::new(Vec::new()),
            context_length: 128_000,
            fallback_text: "(script exhausted)".into(),
            cancel_after_requests: Some(n),
            vision: false,
        })
    }

    /// A provider with a small window, for exercising compaction.
    pub fn with_context_length(turns: Vec<ScriptedTurn>, context_length: u32) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into()),
            seen: Mutex::new(Vec::new()),
            context_length,
            fallback_text: "(script exhausted)".into(),
            cancel_after_requests: None,
            vision: false,
        })
    }

    pub async fn request_count(&self) -> usize {
        self.seen.lock().await.len()
    }

    pub async fn last_request(&self) -> Option<ChatRequest> {
        self.seen.lock().await.last().cloned()
    }
}

#[async_trait]
impl Provider for FakeProvider {
    fn id(&self) -> &str {
        "fake"
    }

    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: "fake".into(),
            display_name: "Fake".into(),
            context_length: Some(self.context_length),
        }])
    }

    async fn capabilities(&self, _model: &str) -> Result<Capabilities> {
        Ok(Capabilities {
            native_tools: true,
            vision: self.vision,
            thinking: false,
            context_length: self.context_length,
        })
    }

    async fn stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> Result<StopReason> {
        // Counted before the request is filed, because what a backend charges
        // for is the whole thing it was sent — the system prompt and every
        // tool schema included, which is the half a session cannot see by
        // looking at its own messages.
        let counted = count_request(&request);
        let served = {
            let mut seen = self.seen.lock().await;
            seen.push(request);
            seen.len()
        };

        if self.cancel_after_requests == Some(served) {
            cancel.cancel();
        }
        if cancel.is_cancelled() {
            return Ok(StopReason::Canceled);
        }

        let turn = self.turns.lock().await.pop_front();
        let turn = turn.unwrap_or_else(|| ScriptedTurn::text(&self.fallback_text));

        for event in turn.events {
            if cancel.is_cancelled() {
                return Ok(StopReason::Canceled);
            }
            if tx.send(event).await.is_err() {
                return Ok(StopReason::Canceled);
            }
        }
        // After the events, so a scripted failure can land either before the
        // stream produced anything or after it produced some of an answer.
        if let Some(error) = turn.failure {
            return Err(error);
        }
        let _ = tx
            .send(StreamEvent::Usage {
                usage: taurus_provider::TokenUsage {
                    input_tokens: counted,
                    output_tokens: 1,
                    ..Default::default()
                },
            })
            .await;
        Ok(turn.stop)
    }
}

/// What a real backend would charge for this request.
///
/// The same four-characters-a-token rule the harness estimates with, over
/// everything the request actually carries. A fake that reported only the
/// messages would agree with the estimate by construction, and the one thing
/// worth testing here is what happens when the two differ.
fn count_request(request: &ChatRequest) -> u32 {
    let system = request.system.as_deref().unwrap_or("").len();
    let tools: usize = request
        .tools
        .iter()
        .map(|t| t.name.len() + t.description.len() + t.input_schema.to_string().len())
        .sum();
    let messages: u32 = request
        .messages
        .iter()
        .map(crate::session::estimate_message)
        .sum();
    messages.saturating_add(((system + tools) / 4) as u32)
}
