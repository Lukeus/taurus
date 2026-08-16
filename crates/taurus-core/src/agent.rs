//! The agent loop.
//!
//! One user message in, one completed assistant turn out, with as many
//! model/tool round trips in between as the task needs. The loop knows nothing
//! about Ollama, Tauri, or React: it drives [`Provider`] and [`ToolRegistry`]
//! and reports progress on a channel.

use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use taurus_provider::prompted::MALFORMED_TOOL;
use taurus_provider::{
    ChatRequest, ContentBlock, Message, Provider, Role, StopReason, StreamAccumulator, StreamEvent,
    TokenUsage,
};
use taurus_tools::{Effect, PlanBoard, ToolContext, ToolError, ToolProgress, ToolRegistry};
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
    /// How many times a request that failed before producing any output is
    /// retried.
    ///
    /// A 429 or a 502 is the backend having a moment, not the turn being wrong,
    /// and losing a session's work to one is a poor trade for the seconds a
    /// retry costs. Only errors [`taurus_provider::ProviderError::is_transient`]
    /// vouches for are retried, and only when nothing has reached the user yet.
    pub max_transient_retries: u32,
    /// First backoff, doubled on each further attempt.
    ///
    /// Configurable rather than a constant so tests can set it to zero; three
    /// real backoffs would put seconds of sleeping into the suite to prove
    /// something that has nothing to do with wall-clock time.
    pub retry_backoff: Duration,
    /// How many times the model may make a tool call that fails, unchanged,
    /// with nothing succeeding in between, before the turn is stopped.
    ///
    /// The system prompt already tells it not to. This is what makes that true:
    /// left alone, a model on a bad path spends the whole iteration budget
    /// rediscovering the same error, and the user waits for all of it to be
    /// told nothing happened.
    ///
    /// Counted across rounds rather than consecutively, so alternating between
    /// two dead ends is caught as readily as insisting on one. Anything that
    /// succeeds resets the count, which is what keeps a model working through
    /// genuinely different candidates from tripping it.
    pub stall_limit: u32,
    /// Whether a turn that changed files without running anything afterwards
    /// gets asked, once, to check its work before it is allowed to finish.
    ///
    /// On by default because the alternative does not work: the system prompt
    /// says to run the tests, and a small model edits a file and stops anyway.
    /// Configurable because it costs a round trip, and a turn that only ever
    /// edits prose has nothing to run.
    pub verify_changes: bool,
}

/// Asked once per turn, when the model stops having changed files it never
/// checked.
///
/// Phrased with a way out. A model that has genuinely nothing to run must be
/// able to say so and finish, or the nudge turns into a round trip spent
/// explaining itself on every documentation edit.
const VERIFY_NUDGE: &str = "\
You changed files and have not run anything since. Check that work now — run \
the project's tests, or build it, or run the thing you changed. If there is \
genuinely nothing to run against it, say so in one line and stop.";

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
            max_transient_retries: 3,
            retry_backoff: Duration::from_millis(500),
            stall_limit: 3,
            verify_changes: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] taurus_provider::ProviderError),
    #[error("reached the {0}-iteration limit for one turn")]
    IterationLimit(u32),
    #[error("the same failing tool call was repeated {0} times with no progress in between")]
    Stalled(u32),
}

/// A request that failed, and whether any of it had already reached the user.
struct FailedAttempt {
    error: taurus_provider::ProviderError,
    produced_output: bool,
}

pub struct Agent {
    provider: Arc<dyn Provider>,
    registry: ToolRegistry,
    tools: ToolContext,
    config: AgentConfig,
    /// The checklist this turn is working through, restated to the model on
    /// every iteration.
    ///
    /// `None` for anything with no `update_plan` tool — every sub-agent, the
    /// examples, most tests — which costs those exactly nothing: no board, no
    /// lock, and not a word added to the prompt. See [`taurus_tools::plan`].
    plan: Option<PlanBoard>,
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
            plan: None,
        }
    }

    /// Gives this agent a checklist, and the prompt that restates it.
    ///
    /// A builder rather than an argument to [`Agent::new`]: the board belongs
    /// to the same caller that registered `update_plan`, and every caller that
    /// did not register it — sub-agents, examples, tests — should not have to
    /// pass a `None` to say so.
    pub fn with_plan(mut self, board: PlanBoard) -> Self {
        self.plan = Some(board);
        self
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
        // Every round since the last sign of progress where *everything*
        // failed, oldest first. Only all-failed rounds are kept: a round where
        // something succeeded is progress, whatever else went wrong alongside
        // it, and it empties this.
        //
        // A list rather than the previous round alone, because a model
        // alternating between two calls that both fail is as stuck as one
        // repeating a single call, and comparing only against the round before
        // would see every round differ from the one before it. Bounded by the
        // iteration ceiling, since anything succeeding clears it.
        let mut failures: Vec<Vec<(String, serde_json::Value)>> = Vec::new();

        // Whether this turn has changed files without running anything since.
        // See `verify_nudge`.
        let recorder = self.tools.checkpoints.clone();
        let mut captured = match &recorder {
            Some(recorder) => recorder.changed_count().await,
            None => 0,
        };
        let mut unverified = false;
        let mut nudged = false;

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
                // The model thinks it is done. If it changed files and never
                // ran anything afterwards, it has not checked its own work —
                // and being told to in the system prompt demonstrably does not
                // make a small model do it. Once per turn, it is asked
                // directly, and the turn continues.
                if unverified && !nudged && self.config.verify_changes {
                    nudged = true;
                    info!("asking the model to check work it has not verified");
                    session.push(Message::user(VERIFY_NUDGE));
                    continue;
                }

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

            // Did this round change anything, and did it check anything?
            //
            // A round that captured new pre-images did work. A round that ran a
            // command and captured nothing asked the project a question and got
            // an answer back — that is what checking looks like from here, and
            // it clears the debt. A command that did both is more work, not a
            // check, so the debt stands.
            if let Some(recorder) = &recorder {
                let now = recorder.changed_count().await;
                let wrote = now > captured;
                captured = now;
                if wrote {
                    unverified = true;
                } else if self.ran_a_check(&assistant) {
                    unverified = false;
                }
            }

            // Checked before the results are pushed, so the transcript ends on
            // the failure the model kept repeating rather than on a stop notice
            // with no visible cause.
            // How many times this exact round has now failed with nothing
            // succeeding in between — not how many times it has arrived in a
            // row. What matters is that the model has been told this answer
            // before and has learned nothing since; whether it tried something
            // else and came back does not change that.
            let repeats = match all_failed(&assistant, &results) {
                Some(calls) => {
                    let seen = failures.iter().filter(|round| **round == calls).count() + 1;
                    failures.push(calls);
                    seen as u32
                }
                None => {
                    failures.clear();
                    0
                }
            };

            session.push(Message::new(Role::User, results));

            if repeats >= self.config.stall_limit {
                let message = format!(
                    "Stopped after the same tool call failed {repeats} times with nothing \
                     succeeding in between. Nothing about it will succeed on a further attempt; a \
                     different approach is needed, or the obstacle needs reporting."
                );
                // Recorded in the transcript, so a resumed session can see why
                // it stopped rather than finding a turn that simply ends.
                session.push(Message::user(message.clone()));
                let _ = ui.send(UiEvent::Error { message }).await;
                info!(repeats, "stopping a stalled turn");
                return Err(AgentError::Stalled(repeats));
            }
        }
    }

    /// One model request, retried while the failure is both transient and
    /// invisible to the user.
    ///
    /// "Invisible" is the load-bearing half. A request that dies on connect can
    /// be sent again and nobody is any the wiser; one that dies half-way
    /// through an answer has already put that half on screen, and retrying it
    /// would write the same paragraph twice. So the moment any text reaches the
    /// UI the request stops being retryable, whatever the error says.
    async fn stream_once(
        &self,
        session: &Session,
        ui: &mpsc::Sender<UiEvent>,
    ) -> Result<(Message, TokenUsage, StopReason), AgentError> {
        let mut attempt = 1;
        loop {
            let failure = match self.stream_attempt(session, ui).await {
                Ok(outcome) => return Ok(outcome),
                Err(failure) => failure,
            };

            let retries_left = attempt <= self.config.max_transient_retries;
            if failure.produced_output
                || !failure.error.is_transient()
                || !retries_left
                || self.tools.cancel.is_cancelled()
            {
                return Err(failure.error.into());
            }

            let _ = ui
                .send(UiEvent::Retrying {
                    attempt: attempt + 1,
                    of: self.config.max_transient_retries + 1,
                    reason: failure.error.to_string(),
                })
                .await;
            info!(attempt, error = %failure.error, "retrying transient provider failure");

            if !self.backoff(attempt).await {
                return Err(failure.error.into());
            }
            attempt += 1;
        }
    }

    /// Sleeps before the next attempt, doubling each time. Returns false if the
    /// turn was canceled while waiting — a user who hits stop during a backoff
    /// should not have to sit through the rest of it.
    async fn backoff(&self, attempt: u32) -> bool {
        // Capped so a large `max_transient_retries` cannot turn into a wait
        // measured in hours.
        let delay = self.config.retry_backoff * 2u32.saturating_pow(attempt.min(6) - 1);
        if delay.is_zero() {
            return !self.tools.cancel.is_cancelled();
        }
        tokio::select! {
            _ = tokio::time::sleep(delay) => true,
            _ = self.tools.cancel.cancelled() => false,
        }
    }

    /// One model request, streamed to the UI and reassembled.
    async fn stream_attempt(
        &self,
        session: &Session,
        ui: &mpsc::Sender<UiEvent>,
    ) -> Result<(Message, TokenUsage, StopReason), FailedAttempt> {
        let request = self.build_request(session).await;
        let (tx, mut rx) = mpsc::channel(128);
        let provider = self.provider.clone();
        let cancel = self.tools.cancel.clone();

        let handle = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

        let mut acc = StreamAccumulator::new();
        // Tool-use deltas do not count: they are accumulated, not displayed, so
        // a stream that dies part-way through a tool call can still be retried
        // without the user seeing anything twice.
        let mut produced_output = false;
        while let Some(event) = rx.recv().await {
            match &event {
                StreamEvent::TextDelta { text } => {
                    produced_output = true;
                    let _ = ui.send(UiEvent::TextDelta { text: text.clone() }).await;
                }
                StreamEvent::ThinkingDelta { text } => {
                    produced_output = true;
                    let _ = ui.send(UiEvent::ThinkingDelta { text: text.clone() }).await;
                }
                _ => {}
            }
            acc.push(event);
        }

        let joined = handle.await.unwrap_or_else(|e| {
            Err(taurus_provider::ProviderError::Protocol(format!(
                "stream task failed: {e}"
            )))
        });
        let stop = joined.map_err(|error| FailedAttempt {
            error,
            produced_output,
        })?;

        let (message, usage, malformed) = acc.finish();
        if !malformed.is_empty() {
            warn!(?malformed, "model produced unparseable tool input");
        }
        Ok((message, usage, stop))
    }

    /// Assembles the request for one iteration.
    ///
    /// Async only because of the plan: the checklist is re-read here rather
    /// than captured when the turn started, which is the entire point of it.
    /// A model on iteration nine gets the plan as it stands on iteration nine,
    /// including the step it marked done on iteration eight.
    async fn build_request(&self, session: &Session) -> ChatRequest {
        let tools = if self.config.allowed_tools.is_empty() {
            self.registry.definitions()
        } else {
            self.registry.definitions_for(&self.config.allowed_tools)
        };

        ChatRequest {
            model: session.model.clone(),
            system: self.system_prompt().await,
            messages: session.messages.clone(),
            tools,
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            stop_sequences: Vec::new(),
        }
    }

    /// The system prompt for this iteration, with the plan appended when there
    /// is one.
    ///
    /// Appended to the system prompt rather than pushed as another message,
    /// for two reasons. A message would be part of the history — compacted,
    /// trimmed, and eventually summarized away, which is exactly the drift the
    /// plan exists to survive. And a second copy would accumulate: one per
    /// iteration, each slightly staler than the last, until the model is
    /// reading nine versions of its own checklist and has to work out which one
    /// is current. The system prompt is rebuilt every iteration, so there is
    /// only ever one, and it is always the live one.
    async fn system_prompt(&self) -> Option<String> {
        let base = self.config.system_prompt.trim();
        let plan = match &self.plan {
            Some(board) => board.reminder().await,
            None => None,
        };

        match (base.is_empty(), plan) {
            (true, None) => None,
            (true, Some(plan)) => Some(plan),
            (false, None) => Some(self.config.system_prompt.clone()),
            // Last, so it is the nearest thing to the conversation. A model
            // that reads the end of its prompt most closely is reading the
            // thing this feature exists to put there.
            (false, Some(plan)) => Some(format!("{}\n\n{plan}", self.config.system_prompt)),
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
            let tool = self.registry.get(name);
            let preview = tool
                .as_ref()
                .map(|t| t.preview(input))
                .unwrap_or_else(|| format!("{name} {input}"));
            let view = tool.and_then(|t| t.view(id, input));
            let _ = ui
                .send(UiEvent::ToolCallStarted {
                    id: id.clone(),
                    name: name.clone(),
                    preview,
                    view,
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

    /// Whether this round ran something that could answer a question about the
    /// project — a build, a test run, a script.
    ///
    /// Keyed off the same declaration the checkpoint sweep uses, so "the tools
    /// that run a program" has one definition rather than a list here that
    /// quietly falls behind the registry.
    fn ran_a_check(&self, assistant: &Message) -> bool {
        assistant.tool_uses().any(|(_, name, _)| {
            self.registry
                .get(name)
                .is_some_and(|tool| tool.touches_unpredictably())
        })
    }

    /// This turn's tool context, bound to one call so anything it reports lands
    /// on that call's card rather than loose in the transcript.
    fn context_for(&self, id: &str, ui: &mpsc::Sender<UiEvent>) -> ToolContext {
        self.tools
            .clone()
            .with_progress(Arc::new(CallProgress {
                id: id.to_string(),
                ui: ui.clone(),
            }))
            .with_call_id(id)
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

    /// Makes room in the context window when it fills up.
    ///
    /// Local models often have 8k windows, so this is load-bearing rather than
    /// a long-session nicety. It works in two tiers, cheapest first:
    ///
    /// 1. Shorten old and superseded tool output. Free — no model call — and
    ///    it usually recovers the most, because tool output is most of what a
    ///    working session holds.
    /// 2. Summarize the older half of the conversation. Costs a request and
    ///    loses detail, so it only runs if step one did not get under budget.
    ///
    /// A failed summarization is not fatal: the turn proceeds uncompacted and
    /// the provider reports the overflow if there is one.
    async fn compact_if_needed(&self, session: &mut Session, ui: &mpsc::Sender<UiEvent>) {
        let Ok(caps) = self.provider.capabilities(&session.model).await else {
            return;
        };
        let budget = (caps.context_length as f32 * self.config.compaction_threshold) as u32;
        if session.estimated_tokens() < budget {
            return;
        }

        // Only a read-only tool answers the same question twice: a repeat of
        // anything else is the model watching the world change, and the two
        // results are both worth keeping. An unregistered name — an MCP server
        // that went away, a transcript from another build — reads as unsafe.
        let registry = &self.registry;
        let repeats_supersede = |name: &str| {
            registry
                .get(name)
                .is_some_and(|tool| tool.effect() == Effect::Read)
        };
        let trimmed =
            session.trim_tool_results(self.config.keep_recent_messages, &repeats_supersede);
        if !trimmed.is_empty() {
            info!(
                results = trimmed.results,
                tokens_saved = trimmed.tokens_saved,
                "trimmed older tool output"
            );
            let _ = ui
                .send(UiEvent::ContextTrimmed {
                    results: trimmed.results,
                    tokens_saved: trimmed.tokens_saved,
                })
                .await;
        }
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

/// This round's calls, as name and arguments, if every one of them failed.
///
/// `None` when anything succeeded and when there were no calls at all: both are
/// progress, and neither should count toward a stall. Call ids are deliberately
/// not part of the identity — the model mints a fresh one each time, so two
/// genuinely identical calls never share one.
fn all_failed(
    assistant: &Message,
    results: &[ContentBlock],
) -> Option<Vec<(String, serde_json::Value)>> {
    let failed: std::collections::HashSet<&str> = results
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error: true,
                ..
            } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();

    let mut calls = Vec::new();
    for (id, name, input) in assistant.tool_uses() {
        if !failed.contains(id) {
            return None;
        }
        calls.push((name.to_string(), input.clone()));
    }
    (!calls.is_empty()).then_some(calls)
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
