//! The agent loop.
//!
//! One user message in, one completed assistant turn out, with as many
//! model/tool round trips in between as the task needs. The loop knows nothing
//! about Ollama, Tauri, or React: it drives [`Provider`] and [`ToolRegistry`]
//! and reports progress on a channel.

use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use taurus_provider::prompted::MALFORMED_TOOL;
use taurus_provider::{
    ChatRequest, ContentBlock, Message, Provider, Role, StopReason, StreamAccumulator, StreamEvent,
    TokenUsage,
};
use taurus_tools::{ToolContext, ToolError, ToolProgress, ToolRegistry};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::event::{truncate_for_ui, UiEvent};
use crate::session::{split_for_compaction, Session};

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub system_prompt: String,
    /// Ceiling on tool round trips in a single turn. Stops a model that has
    /// gotten stuck in a call/retry cycle from running forever.
    pub max_iterations: u32,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Fraction of the context window at which history gets summarized.
    pub compaction_threshold: f32,
    /// Messages kept verbatim when compacting.
    pub keep_recent_messages: usize,
    /// Tools the model may call. Empty means every registered tool.
    pub allowed_tools: Vec<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            max_iterations: 25,
            temperature: None,
            max_tokens: None,
            compaction_threshold: 0.8,
            keep_recent_messages: 8,
            allowed_tools: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] taurus_provider::ProviderError),
    #[error("reached the {0}-iteration limit for one turn")]
    IterationLimit(u32),
}

pub struct Agent {
    provider: Arc<dyn Provider>,
    registry: ToolRegistry,
    tools: ToolContext,
    config: AgentConfig,
}

/// What a completed turn produced.
#[derive(Debug)]
pub struct TurnOutcome {
    pub stop_reason: StopReason,
    pub usage: TokenUsage,
    pub iterations: u32,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        registry: ToolRegistry,
        tools: ToolContext,
        config: AgentConfig,
    ) -> Self {
        Self {
            provider,
            registry,
            tools,
            config,
        }
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Runs one turn to completion.
    ///
    /// `session` is mutated in place, so a canceled or failed turn still leaves
    /// the history consistent and resumable.
    pub async fn run_turn(
        &self,
        session: &mut Session,
        user_message: Message,
        ui: mpsc::Sender<UiEvent>,
    ) -> Result<TurnOutcome, AgentError> {
        session.push(user_message);

        let mut total = TokenUsage::default();
        let mut iteration = 0;

        loop {
            if self.tools.cancel.is_cancelled() {
                let _ = ui
                    .send(UiEvent::TurnFinished {
                        stop_reason: StopReason::Canceled,
                        usage: total,
                    })
                    .await;
                return Ok(TurnOutcome {
                    stop_reason: StopReason::Canceled,
                    usage: total,
                    iterations: iteration,
                });
            }

            iteration += 1;
            if iteration > self.config.max_iterations {
                // Tell the model why it is being cut off, in the transcript,
                // so a resumed session has the context.
                session.push(Message::user(format!(
                    "Stopped after {} tool round trips without finishing. Summarize what you \
                     learned and what remains.",
                    self.config.max_iterations
                )));
                let _ = ui
                    .send(UiEvent::Error {
                        message: format!(
                            "Stopped after {} tool round trips.",
                            self.config.max_iterations
                        ),
                    })
                    .await;
                return Err(AgentError::IterationLimit(self.config.max_iterations));
            }

            self.compact_if_needed(session, &ui).await;
            let _ = ui.send(UiEvent::IterationStarted { iteration }).await;

            let (assistant, usage, stop) = self.stream_once(session, &ui).await?;
            total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
            session.add_usage(usage);

            let has_tools = assistant.has_tool_use();
            if !assistant.content.is_empty() {
                session.push(assistant.clone());
            }

            if stop == StopReason::Canceled {
                let _ = ui
                    .send(UiEvent::TurnFinished {
                        stop_reason: stop,
                        usage: total,
                    })
                    .await;
                return Ok(TurnOutcome {
                    stop_reason: stop,
                    usage: total,
                    iterations: iteration,
                });
            }

            // A provider can report ToolUse without emitting a parseable call,
            // and a prompted model can emit one without the provider noticing.
            // The message itself is the authority.
            if !has_tools {
                let _ = ui
                    .send(UiEvent::TurnFinished {
                        stop_reason: stop,
                        usage: total,
                    })
                    .await;
                return Ok(TurnOutcome {
                    stop_reason: stop,
                    usage: total,
                    iterations: iteration,
                });
            }

            let results = self.run_tool_calls(&assistant, &ui).await;
            session.push(Message::new(Role::User, results));
        }
    }

    /// One model request, streamed to the UI and reassembled.
    async fn stream_once(
        &self,
        session: &Session,
        ui: &mpsc::Sender<UiEvent>,
    ) -> Result<(Message, TokenUsage, StopReason), AgentError> {
        let request = self.build_request(session);
        let (tx, mut rx) = mpsc::channel(128);
        let provider = self.provider.clone();
        let cancel = self.tools.cancel.clone();

        let handle = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

        let mut acc = StreamAccumulator::new();
        while let Some(event) = rx.recv().await {
            match &event {
                StreamEvent::TextDelta { text } => {
                    let _ = ui.send(UiEvent::TextDelta { text: text.clone() }).await;
                }
                StreamEvent::ThinkingDelta { text } => {
                    let _ = ui.send(UiEvent::ThinkingDelta { text: text.clone() }).await;
                }
                _ => {}
            }
            acc.push(event);
        }

        let stop = handle.await.map_err(|e| {
            taurus_provider::ProviderError::Protocol(format!("stream task failed: {e}"))
        })??;

        let (message, usage, malformed) = acc.finish();
        if !malformed.is_empty() {
            warn!(?malformed, "model produced unparseable tool input");
        }
        Ok((message, usage, stop))
    }

    fn build_request(&self, session: &Session) -> ChatRequest {
        let tools = if self.config.allowed_tools.is_empty() {
            self.registry.definitions()
        } else {
            self.registry.definitions_for(&self.config.allowed_tools)
        };

        ChatRequest {
            model: session.model.clone(),
            system: (!self.config.system_prompt.trim().is_empty())
                .then(|| self.config.system_prompt.clone()),
            messages: session.messages.clone(),
            tools,
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            stop_sequences: Vec::new(),
        }
    }

    /// Executes every tool call in an assistant message and returns the results
    /// in call order.
    ///
    /// Read-only calls run concurrently, since a model that asks for six files
    /// at once should not wait six round trips. Anything with a side effect is
    /// serialized so the outcome does not depend on scheduling.
    async fn run_tool_calls(
        &self,
        assistant: &Message,
        ui: &mpsc::Sender<UiEvent>,
    ) -> Vec<ContentBlock> {
        let calls: Vec<(String, String, serde_json::Value)> = assistant
            .tool_uses()
            .map(|(id, name, input)| (id.to_string(), name.to_string(), input.clone()))
            .collect();

        for (id, name, input) in &calls {
            let preview = self
                .registry
                .get(name)
                .map(|t| t.preview(input))
                .unwrap_or_else(|| format!("{name} {input}"));
            let _ = ui
                .send(UiEvent::ToolCallStarted {
                    id: id.clone(),
                    name: name.clone(),
                    preview,
                })
                .await;
        }

        let (concurrent, sequential): (Vec<_>, Vec<_>) =
            calls.into_iter().partition(|(_, n, _)| {
                self.registry
                    .get(n)
                    .is_some_and(|t| t.effect().is_concurrent_safe())
            });

        let mut results: Vec<(String, ContentBlock)> = Vec::new();

        let mut pending: FuturesUnordered<_> = concurrent
            .into_iter()
            .map(|(id, name, input)| {
                let ctx = self.context_for(&id, ui);
                async move {
                    let outcome = self.execute_one(&name, input, &ctx).await;
                    (id, outcome)
                }
            })
            .collect();
        while let Some((id, outcome)) = pending.next().await {
            results.push((id.clone(), self.report(&id, outcome, ui).await));
        }

        for (id, name, input) in sequential {
            let ctx = self.context_for(&id, ui);
            let outcome = self.execute_one(&name, input, &ctx).await;
            results.push((id.clone(), self.report(&id, outcome, ui).await));
        }

        // Restore the model's original call order: some providers reject
        // results that arrive out of sequence, and it reads better in the log.
        let order: Vec<&str> = assistant.tool_uses().map(|(id, _, _)| id).collect();
        let mut ordered = Vec::with_capacity(results.len());
        for id in order {
            if let Some(pos) = results.iter().position(|(rid, _)| rid == id) {
                ordered.push(results.remove(pos).1);
            }
        }
        ordered.extend(results.into_iter().map(|(_, block)| block));
        ordered
    }

    /// This turn's tool context, bound to one call so anything it reports lands
    /// on that call's card rather than loose in the transcript.
    fn context_for(&self, id: &str, ui: &mpsc::Sender<UiEvent>) -> ToolContext {
        self.tools.clone().with_progress(Arc::new(CallProgress {
            id: id.to_string(),
            ui: ui.clone(),
        }))
    }

    async fn execute_one(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, ToolError> {
        // The prompted fallback surfaces syntax failures under this name. A
        // bare "no such tool" would send the model looking for a different
        // tool instead of fixing its formatting.
        if name == MALFORMED_TOOL {
            return Err(ToolError::InvalidInput(
                "That tool call could not be parsed. Emit exactly one JSON object between \
                 <tool_call> and </tool_call>, of the form {\"name\": ..., \"input\": {...}}."
                    .into(),
            ));
        }
        self.registry.execute(name, input, ctx).await
    }

    async fn report(
        &self,
        id: &str,
        outcome: Result<String, ToolError>,
        ui: &mpsc::Sender<UiEvent>,
    ) -> ContentBlock {
        match outcome {
            Ok(output) => {
                let _ = ui
                    .send(UiEvent::ToolCallFinished {
                        id: id.to_string(),
                        ok: true,
                        output: truncate_for_ui(&output),
                    })
                    .await;
                ContentBlock::tool_result(id, output)
            }
            Err(error) => {
                let message = error.to_model_message();
                debug!(id, %error, "tool call failed");
                let _ = ui
                    .send(UiEvent::ToolCallFinished {
                        id: id.to_string(),
                        ok: false,
                        output: truncate_for_ui(&message),
                    })
                    .await;
                ContentBlock::tool_error(id, message)
            }
        }
    }

    /// Summarizes older history when the window fills up.
    ///
    /// Local models often have 8k windows, so this is load-bearing rather than
    /// a long-session nicety. A failed summarization is not fatal: the turn
    /// proceeds uncompacted and the provider reports the overflow if there is
    /// one.
    async fn compact_if_needed(&self, session: &mut Session, ui: &mpsc::Sender<UiEvent>) {
        let Ok(caps) = self.provider.capabilities(&session.model).await else {
            return;
        };
        let budget = (caps.context_length as f32 * self.config.compaction_threshold) as u32;
        if session.estimated_tokens() < budget {
            return;
        }

        let (drop_count, _) =
            split_for_compaction(&session.messages, self.config.keep_recent_messages);
        if drop_count == 0 {
            return;
        }

        info!(drop_count, "compacting session history");
        let older: Vec<Message> = session.messages[..drop_count].to_vec();
        let Some(summary) = self.summarize(&session.model, older).await else {
            warn!("compaction failed; continuing with full history");
            return;
        };

        let mut rest = session.messages.split_off(drop_count);
        session.messages.clear();
        session.messages.push(Message::user(format!(
            "Summary of the earlier conversation:\n\n{summary}"
        )));
        session.messages.append(&mut rest);

        let _ = ui
            .send(UiEvent::Compacted {
                messages_removed: drop_count,
            })
            .await;
    }

    async fn summarize(&self, model: &str, messages: Vec<Message>) -> Option<String> {
        let mut request = ChatRequest::new(model, messages);
        request.system = Some(
            "Summarize the conversation so far for your own future reference. Preserve: the \
             user's goal, decisions made, files read or changed, and anything still outstanding. \
             Drop pleasantries and superseded detail. Write prose, under 400 words."
                .into(),
        );
        request.messages.push(Message::user(
            "Write that summary now, as described in the system prompt.",
        ));

        let (tx, mut rx) = mpsc::channel(64);
        let provider = self.provider.clone();
        let cancel = self.tools.cancel.clone();
        let handle = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

        let mut acc = StreamAccumulator::new();
        while let Some(event) = rx.recv().await {
            acc.push(event);
        }
        handle.await.ok()?.ok()?;

        let text = acc.finish().0.text();
        (!text.trim().is_empty()).then_some(text)
    }
}

/// Forwards a tool's progress reports to the UI, tagged with the call they
/// belong to.
struct CallProgress {
    id: String,
    ui: mpsc::Sender<UiEvent>,
}

#[async_trait::async_trait]
impl ToolProgress for CallProgress {
    async fn step(&self, label: String) {
        // Dropped rather than allowed to stall the tool if the UI is gone or
        // behind: progress is by definition the part that can be missed.
        let _ = self
            .ui
            .send(UiEvent::ToolProgress {
                id: self.id.clone(),
                label,
            })
            .await;
    }
}
