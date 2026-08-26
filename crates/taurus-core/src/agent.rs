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
use tracing::{debug, info, warn, Instrument};

use crate::event::{truncate_for_ui, ResultImage, UiEvent};
use crate::session::{split_for_compaction, Session};

/// How long consecutive deltas of one kind are gathered before being handed on.
///
/// Every token a model produces used to be its own `UiEvent`, which is its own
/// `serde_json` serialize, its own webview message and its own JS dispatch. A
/// small local model streams 200–500 tokens a second, so that was hundreds of
/// crossings a second competing for the same main thread the tokens are being
/// rendered on — jank on exactly the fastest setups.
///
/// Chosen against what the frontend does with them rather than by feel: it
/// coalesces events into one render per 30ms frame, so anything gathered inside
/// that window would have waited for the same frame anyway. The cost is that
/// the *first* token of a turn is handed on up to this late; a frame is the
/// smallest unit the screen can show it in either way.
const COALESCE: Duration = Duration::from_millis(16);

/// Deltas of one kind, gathered but not yet handed on.
struct Held {
    thinking: bool,
    text: String,
    /// When this has to go, whatever else has happened by then.
    due: std::time::Instant,
}

impl Held {
    fn new(thinking: bool, text: &str) -> Self {
        Self {
            thinking,
            text: text.to_string(),
            due: std::time::Instant::now() + COALESCE,
        }
    }

    async fn send(self, ui: &mpsc::Sender<UiEvent>) {
        let event = if self.thinking {
            UiEvent::ThinkingDelta { text: self.text }
        } else {
            UiEvent::TextDelta { text: self.text }
        };
        let _ = ui.send(event).await;
    }
}

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
    /// Whether a trace may carry the conversation itself, not only its shape.
    ///
    /// [`Capture::MetadataOnly`] by default, and that default is the feature:
    /// turning telemetry on says how much a turn cost, not what was in it. See
    /// [`crate::telemetry::Capture`].
    pub capture: crate::telemetry::Capture,
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

/// Asked once per turn, when the model stops with steps still open on its plan.
///
/// The failure it exists for is not a model that gave up half way. It is a
/// model that did all the work, marked the last step `active`, ran it, and then
/// reported finishing *in prose* — leaving a checklist that says 2 of 3 and a
/// panel that will still say so tomorrow. Telling it in the prompt to close the
/// list does not fix this any more than telling it to run the tests fixes the
/// other one: the plan is right there in the system prompt on every iteration,
/// and it stops anyway.
///
/// Phrased with the same way out as [`VERIFY_NUDGE`], for the same reason. A
/// turn that genuinely stopped early — a question to ask, work it cannot do —
/// has to be able to say so and finish, or the nudge becomes a round trip spent
/// re-explaining a plan the model already abandoned on purpose.
const PLAN_NUDGE: &str = "\
Your plan still has steps that are not marked done. If you finished them, call \
update_plan now with the whole list and every finished step marked 'done'. If \
you have stopped without finishing them, say why in one line and stop.";

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
            capture: crate::telemetry::Capture::MetadataOnly,
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
    /// A `user_prompt_submit` hook refused the message.
    ///
    /// Its own variant rather than a provider error because nothing went wrong:
    /// the user configured a check and the check said no, and the message the
    /// hook wrote is the whole of what there is to report.
    #[error("{0}")]
    Refused(String),
}

impl AgentError {
    /// A stable, low-cardinality name for this kind of failure, for
    /// `error.type` on the turn's span.
    ///
    /// A refused turn and a stalled one are the two worth charting: the first
    /// is a hook doing its job, the second is a model that could not make
    /// progress, and reading a rise in either as "errors are up" would send
    /// somebody looking in the wrong place.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Provider(error) => error.kind(),
            Self::IterationLimit(_) => "iteration_limit",
            Self::Stalled(_) => "stalled",
            Self::Refused(_) => "refused_by_hook",
        }
    }
}

/// The history with every image replaced by a line saying one was there.
///
/// For a conversation that has moved to a model which cannot see. The images
/// stay in the session and in the transcript — this is only what is sent — so
/// moving back to a vision model brings them back. Nothing else about the
/// conversation is lost, which is the whole reason a switch is allowed to
/// happen in a conversation that has pictures in it.
///
/// Replaced rather than dropped, because the text around an image usually
/// refers to it. A message reading "what is wrong with this?" with the picture
/// silently removed is a question about nothing; one that says an image was
/// omitted is a question the model can answer honestly. It also keeps every
/// message's content non-empty, which some providers require.
fn without_images(messages: &[Message]) -> Vec<Message> {
    // The common case by far: nothing to do, and no history copied twice.
    if !messages.iter().any(|m| {
        m.content
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. }))
    }) {
        return messages.to_vec();
    }

    messages
        .iter()
        .map(|message| Message {
            role: message.role,
            content: message
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Image { .. } => ContentBlock::text(IMAGE_OMITTED),
                    other => other.clone(),
                })
                .collect(),
        })
        .collect()
}

/// What stands in for a picture the current model cannot read.
const IMAGE_OMITTED: &str = "[an image was attached here; this model cannot read images]";

/// A request that failed, and whether any of it had already reached the user.
struct FailedAttempt {
    error: taurus_provider::ProviderError,
    produced_output: bool,
}

/// Where a turn's conversation is written down as it happens.
///
/// The agent drives this rather than owning it, because persistence is a
/// question about *where a session lives* — a directory, a format, a version —
/// and this crate is the one that must not know the answer. `taurus-host`
/// implements it over a transcript file.
///
/// Called once per tool round trip and once when the turn ends, however it
/// ends, each time with the whole session. Implementations append what is new
/// rather than rewriting, which is what makes the repetition cheap and the
/// crash-in-the-middle case survivable.
///
/// Recording must never fail a turn: an implementation that cannot write
/// swallows the error, exactly as the host's own transcript log does for the
/// conversation the user is having.
#[async_trait::async_trait]
pub trait TurnRecorder: Send + Sync {
    async fn record(&self, session: &Session);
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
    /// Where this turn is written down, when somebody is keeping it. `None` for
    /// the top-level agent, whose transcript its frontend records itself.
    recorder: Option<Arc<dyn TurnRecorder>>,
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
            recorder: None,
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

    /// Writes this agent's turns down as they happen.
    ///
    /// A builder for the same reason [`Agent::with_plan`] is: the caller that
    /// has somewhere to put a transcript is not every caller, and the ones
    /// without should not have to pass a `None` to say so.
    pub fn with_recorder(mut self, recorder: Arc<dyn TurnRecorder>) -> Self {
        self.recorder = Some(recorder);
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
        // Before the message is pushed, so a hook that refuses one leaves the
        // conversation exactly as it was rather than with a message in it that
        // was never answered.
        if let Some(refusal) = self.before_prompt(&user_message).await {
            let _ = ui
                .send(UiEvent::Error {
                    message: refusal.clone(),
                })
                .await;
            return Err(AgentError::Refused(refusal));
        }

        // Parent to every model call and tool call the turn makes — and, for
        // free, to a delegate's, because a sub-agent runs inside the `spawn`
        // tool's span. A nine-step turn that delegated twice reads as a tree
        // rather than a flat list somebody reassembles by timestamp.
        let span = crate::telemetry::turn_span(self.provider.id(), &session.model, &session.id);
        let outcome = self
            .turn(session, user_message, ui)
            .instrument(span.clone())
            .await;
        match &outcome {
            Ok(finished) => {
                crate::telemetry::record_usage(&span, &finished.usage);
                span.record(
                    "gen_ai.response.finish_reasons",
                    crate::telemetry::finish_reason(finished.stop_reason),
                );
            }
            Err(error) => crate::telemetry::record_error(&span, error.kind()),
        }
        // Here rather than at each `return` inside `turn`: it ends at half a
        // dozen of them — finished, canceled twice, out of iterations, stalled,
        // and any provider error — and the one that would eventually be
        // forgotten is the one that matters most. A turn that failed is a turn
        // somebody will want to read.
        self.persist(session).await;
        // After the transcript is written, not before. A `stop` hook that reads
        // the session — which is most of the reason to write one — must see the
        // turn that just ended rather than the one before it.
        self.after_turn().await;
        outcome
    }

    /// Runs the `user_prompt_submit` hooks, and reports a refusal.
    ///
    /// A hook that refuses stops the turn before it starts. A hook that passes
    /// and prints something has that carried into the message the model reads,
    /// which is what makes "attach the current branch to every prompt" a
    /// three-line script rather than a feature.
    async fn before_prompt(&self, message: &Message) -> Option<String> {
        let runner = self.tools.hooks.as_ref()?;
        if !runner.has(taurus_hooks::HookEvent::UserPromptSubmit) {
            return None;
        }

        let mut payload = taurus_hooks::HookPayload::new(
            taurus_hooks::HookEvent::UserPromptSubmit,
            &self.tools.workspace,
        )
        .with_prompt(message.text());
        if let Some(session) = &self.tools.session_id {
            payload = payload.with_session(session.clone());
        }
        runner.run(&payload).await.denied
    }

    /// Runs the `stop` hooks. Nothing can be refused here — the turn is over —
    /// so anything they say goes to the log rather than into a conversation
    /// that has already been answered.
    async fn after_turn(&self) {
        let Some(runner) = self.tools.hooks.as_ref() else {
            return;
        };
        if !runner.has(taurus_hooks::HookEvent::Stop) {
            return;
        }

        let mut payload =
            taurus_hooks::HookPayload::new(taurus_hooks::HookEvent::Stop, &self.tools.workspace);
        if let Some(session) = &self.tools.session_id {
            payload = payload.with_session(session.clone());
        }
        for note in runner.run(&payload).await.notes {
            info!(%note, "stop hook");
        }
    }

    async fn persist(&self, session: &Session) {
        if let Some(recorder) = &self.recorder {
            recorder.record(session).await;
        }
    }

    async fn turn(
        &self,
        session: &mut Session,
        user_message: Message,
        ui: mpsc::Sender<UiEvent>,
    ) -> Result<TurnOutcome, AgentError> {
        session.push(user_message);
        // Before the first request, not after the last one. Two things follow
        // from writing the question down at the moment it is asked rather than
        // when its answer is complete: a turn interrupted by a crash or a
        // kill leaves the question on disk instead of nothing, and the
        // conversation becomes listable — with a title — while it is being
        // answered rather than once it has been. A turn that takes two minutes
        // was previously absent from every listing for those two minutes.
        self.persist(session).await;

        let mut total = TokenUsage::default();
        let mut iteration = 0;
        // Every round since the last sign of progress where *everything*
        // failed, oldest first — each one as the answers it got, not the
        // arguments it sent; see `all_failed`. Only all-failed rounds are kept:
        // a round where something succeeded is progress, whatever else went
        // wrong alongside it, and it empties this.
        //
        // A list rather than the previous round alone, because a model
        // alternating between two calls that both fail is as stuck as one
        // repeating a single call, and comparing only against the round before
        // would see every round differ from the one before it. Bounded by the
        // iteration ceiling, since anything succeeding clears it.
        let mut failures: Vec<Vec<(String, String)>> = Vec::new();

        // Whether this turn has changed files without running anything since.
        // See `verify_nudge`.
        let recorder = self.tools.checkpoints.clone();
        let mut captured = match &recorder {
            Some(recorder) => recorder.changed_count().await,
            None => 0,
        };
        let mut unverified = false;
        // A summarization that failed will fail again on the next iteration for
        // the same reason — the provider, the model, or the shape of the
        // request — and a session over budget asks on every one of them. One
        // broken request used to become `max_iterations` of them, which is what
        // made a single 404 look like a storm.
        let mut summarizing = Summarizing::Allowed;
        let mut nudged = false;
        // Tracked separately from `nudged`: they answer different questions and
        // a turn can earn both. See `PLAN_NUDGE`.
        let mut plan_nudged = false;

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

            summarizing = self.compact_if_needed(session, &ui, summarizing).await;
            let _ = ui.send(UiEvent::IterationStarted { iteration }).await;

            let (assistant, usage, stop) = self.stream_once(session, &ui).await?;
            total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
            // Before the answer is pushed: what the provider counted is what
            // was sent, and the estimate it is paired with has to be of the
            // same messages. See `Session::record_request`.
            session.record_request(usage.input_tokens);
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

                // After the verify nudge, not before: checking the work can
                // change what the plan should say, and a turn asked to close
                // its list and then told to go and run something would have
                // closed it one round too early.
                if !plan_nudged && self.plan.as_ref().is_some_and(|b| b.unfinished()) {
                    plan_nudged = true;
                    info!("asking the model to close the steps it left open");
                    session.push(Message::user(PLAN_NUDGE));
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
            // A round that captured new pre-images did work, and work nothing
            // has been run against is the debt the nudge exists to collect.
            // Running something clears it — but only when nothing was written
            // afterwards, because the word the nudge uses is *since*.
            if let Some(recorder) = &recorder {
                let now = recorder.changed_count().await;
                let wrote = now > captured;
                captured = now;
                if wrote {
                    // Only when the count moved. The set is read off a lock the
                    // tools are also using, and a round that changed nothing
                    // has nothing to tell anybody.
                    let _ = ui
                        .send(UiEvent::FilesChanged {
                            paths: recorder.changed_paths().await,
                        })
                        .await;
                }
                // Order first, and the file count only as a fallback. This used
                // to read "wrote something, so the debt stands; otherwise, ran
                // something, so it is cleared" — which cannot see that the
                // thing that wrote *was* the check. A test runner leaves a
                // `.coverage` behind, and the sweep looks past a file an ignore
                // rule excludes on purpose, so the run counted as work and the
                // model was told it had not run anything since — one line after
                // it ran the tests and reported them passing.
                if self.checked_with_nothing_written_after(&assistant) {
                    unverified = false;
                } else if wrote {
                    unverified = true;
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
            // Once a round, so a long delegation that dies half way through
            // leaves the rounds it did finish rather than nothing at all.
            self.persist(session).await;

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
        // Asked per attempt rather than held, because the model this session is
        // on can change between turns — see `Session::model`. Cached by every
        // adapter, so after the first turn this is a map lookup.
        //
        // `true` when the answer cannot be had: stripping images from a
        // conversation on the strength of a failed capability lookup would
        // quietly degrade a session that was working.
        let vision = self
            .provider
            .capabilities(&session.model)
            .await
            .map(|caps| caps.vision)
            .unwrap_or(true);

        let request = self.build_request(session, vision);

        // Per attempt rather than per iteration: a request that 429'd and was
        // retried is two round trips against the backend and, on a metered
        // provider, potentially two bills. One span covering both would be
        // telling a story about the backend that did not happen.
        let span = crate::telemetry::chat_span(self.provider.id(), &session.model, &session.id);
        if self.config.capture.content() {
            // Serialized only when asked. The messages are the workspace — the
            // files read, the commands run, whatever was pasted in — and the
            // cost of building this string is the smaller reason not to.
            if let Ok(messages) = serde_json::to_string(&request.messages) {
                span.record("gen_ai.input.messages", messages.as_str());
            }
        }
        let _entered = span.enter();

        let (tx, mut rx) = mpsc::channel(128);
        let provider = self.provider.clone();
        let cancel = self.tools.cancel.clone();

        let handle = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

        let mut acc = StreamAccumulator::new();
        // Tool-use deltas do not count: they are accumulated, not displayed, so
        // a stream that dies part-way through a tool call can still be retried
        // without the user seeing anything twice.
        let mut produced_output = false;
        // What has arrived since the last thing was handed on. See `COALESCE`.
        let mut held: Option<Held> = None;

        loop {
            let arrived = match &held {
                // Something is being held back, so this cannot block until the
                // model happens to speak again: a model that stops mid-sentence
                // to think would leave the last few tokens of it unsent.
                Some(h) => match tokio::time::timeout_at(h.due.into(), rx.recv()).await {
                    Ok(arrived) => arrived,
                    Err(_) => {
                        held.take().unwrap().send(ui).await;
                        continue;
                    }
                },
                None => rx.recv().await,
            };
            let Some(event) = arrived else { break };

            let delta = match &event {
                StreamEvent::TextDelta { text } => Some((false, text)),
                StreamEvent::ThinkingDelta { text } => Some((true, text)),
                _ => None,
            };
            if let Some((thinking, text)) = delta {
                produced_output = true;
                match &mut held {
                    // The two kinds are separate messages on screen, so a run
                    // of one cannot absorb the other.
                    Some(h) if h.thinking == thinking => h.text.push_str(text),
                    Some(_) => {
                        held.take().unwrap().send(ui).await;
                        held = Some(Held::new(thinking, text));
                    }
                    None => held = Some(Held::new(thinking, text)),
                }
            }
            acc.push(event);
        }

        // Whatever the stream ended mid-window with. Before the error is
        // looked at, so a stream that failed part-way still shows what it did
        // manage to say — which is also what `produced_output` promises.
        if let Some(h) = held.take() {
            h.send(ui).await;
        }

        let joined = handle.await.unwrap_or_else(|e| {
            Err(taurus_provider::ProviderError::Protocol(format!(
                "stream task failed: {e}"
            )))
        });
        let stop = match joined {
            Ok(stop) => stop,
            Err(error) => {
                // By type, not by message: `error.type` is meant to be
                // something a dashboard can group by, and the message is
                // already on the log event beside it.
                crate::telemetry::record_error(&span, error.kind());
                return Err(FailedAttempt {
                    error,
                    produced_output,
                });
            }
        };

        let (message, usage, malformed) = acc.finish();
        if !malformed.is_empty() {
            warn!(?malformed, "model produced unparseable tool input");
        }

        span.record("gen_ai.response.model", session.model.as_str());
        span.record(
            "gen_ai.response.finish_reasons",
            crate::telemetry::finish_reason(stop),
        );
        crate::telemetry::record_usage(&span, &usage);
        if self.config.capture.content() {
            if let Ok(output) = serde_json::to_string(&message) {
                span.record("gen_ai.output.messages", output.as_str());
            }
        }

        Ok((message, usage, stop))
    }

    /// Assembles the request for one iteration.
    ///
    /// The plan is re-read here rather than captured when the turn started,
    /// which is the entire point of it: a model on iteration nine gets the plan
    /// as it stands on iteration nine, including the step it marked done on
    /// iteration eight.
    fn build_request(&self, session: &Session, vision: bool) -> ChatRequest {
        let tools = if self.config.allowed_tools.is_empty() {
            self.registry.definitions()
        } else {
            self.registry.definitions_for(&self.config.allowed_tools)
        };

        let mut messages = if vision {
            session.messages.clone()
        } else {
            without_images(&session.messages)
        };
        self.append_plan(&mut messages);

        ChatRequest {
            model: session.model.clone(),
            system: Some(self.config.system_prompt.clone()).filter(|s| !s.trim().is_empty()),
            messages,
            tools,
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            stop_sequences: Vec::new(),
        }
    }

    /// Puts the live plan at the very end of the request.
    ///
    /// Written onto the copy being sent, never into the session. That is what
    /// keeps the two properties the plan needs: it is not part of the history,
    /// so compaction cannot summarize it away and it cannot drift; and it is
    /// rebuilt every iteration, so there is only ever one copy and it is always
    /// the live one.
    ///
    /// Position is the whole point. This used to hang off the end of the system
    /// prompt, which reads like the end of something and is in fact the very
    /// beginning of the request — ahead of the tool schemas and every message.
    /// A backend serving this reuses the longest identical prefix of a prompt
    /// it has already processed, and `update_plan` is called at the start and
    /// end of every step, so a plan sitting up there invalidated the tools and
    /// the whole conversation each time it moved. Measured on a local 30B, one
    /// 9,550-token prompt: 16ms to repeat unchanged, 10,933ms to repeat with
    /// one line of the plan edited. On Anthropic it is the same fact with a
    /// price on it — the cache breakpoint sits on the system field and covers
    /// the tools rendered before it, and a moved plan misses both.
    ///
    /// At the tail it invalidates only itself, and it is nearer the model's
    /// attention than it was before rather than further away.
    fn append_plan(&self, messages: &mut Vec<Message>) {
        let Some(plan) = self.plan.as_ref().and_then(|board| board.reminder()) else {
            return;
        };
        // Onto the last message rather than after it. Every request is built
        // with a user message last — the person's turn, a round of tool
        // results, or a nudge — and a second one beside it is a shape some of
        // these APIs reject outright.
        match messages.last_mut() {
            Some(last) if last.role == Role::User => last.content.push(ContentBlock::text(plan)),
            _ => messages.push(Message::user(plan)),
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
    /// Keyed off a declaration on the tool rather than a list of names here,
    /// which would quietly fall behind the registry. `Tool::checks_work`
    /// follows the sweep's question by default and parts from it for
    /// `check_command`, where the run being read finished after the turn that
    /// started it.
    /// Whether this round ran something and then wrote nothing after it.
    ///
    /// Calls in one message run in the order they appear, so this is a walk
    /// rather than a pair of `any`s: a command that cannot say what it touches
    /// is the model asking the project a question, and a tool that names the
    /// file it is about to write is the model changing its answer. The last one
    /// of the two decides, exactly as it would across two rounds.
    ///
    /// Which leaves out one case on purpose. A command that both checks and
    /// works — a `make` that builds and formats, a test run that updates its
    /// own snapshots — clears the debt here, because there is nothing in a
    /// shell command to tell the two apart and the alternative is what this
    /// replaced: the harness telling a model that just ran the tests that it
    /// has not run anything. A wrong nudge costs a round trip and says
    /// something false; a missed one leaves a backstop unused, with the system
    /// prompt still asking for the same thing.
    fn checked_with_nothing_written_after(&self, assistant: &Message) -> bool {
        let mut checked = false;
        for (_, name, input) in assistant.tool_uses() {
            let Some(tool) = self.registry.get(name) else {
                continue;
            };
            if tool.checks_work() {
                checked = true;
            } else if !tool.touches(input).is_empty() {
                checked = false;
            }
        }
        checked
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
    ) -> taurus_tools::ToolResult {
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
        let span = crate::telemetry::tool_span(name, ctx.call_id.as_deref().unwrap_or("unknown"));
        let result = self
            .registry
            .execute(name, input, ctx)
            .instrument(span.clone())
            .await;
        if let Err(error) = &result {
            crate::telemetry::record_error(&span, error.kind());
        }
        result
    }

    async fn report(
        &self,
        id: &str,
        outcome: taurus_tools::ToolResult,
        ui: &mpsc::Sender<UiEvent>,
    ) -> ContentBlock {
        match outcome {
            Ok(output) => {
                let _ = ui
                    .send(UiEvent::ToolCallFinished {
                        id: id.to_string(),
                        ok: true,
                        output: truncate_for_ui(&output.to_text()),
                        // Full size, not truncated: an image is not text with a
                        // tail to cut off, and half of a PNG is not half a
                        // picture. The size cap that keeps this bounded is
                        // applied where the result is produced — see
                        // `taurus_tools::registry`.
                        images: output
                            .images()
                            .map(|(mime_type, data)| ResultImage {
                                mime_type: mime_type.to_string(),
                                data: data.to_string(),
                            })
                            .collect(),
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
                        images: Vec::new(),
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
    async fn compact_if_needed(
        &self,
        session: &mut Session,
        ui: &mpsc::Sender<UiEvent>,
        summarizing: Summarizing,
    ) -> Summarizing {
        let Ok(caps) = self.provider.capabilities(&session.model).await else {
            return summarizing;
        };
        let budget = (caps.context_length as f32 * self.config.compaction_threshold) as u32;
        // The messages are the part of the prompt that can be shrunk, and they
        // used to be the whole of what was measured — so the system prompt, the
        // tool schemas, and the plan rode along uncounted, and the threshold
        // was quietly paying for them. It cannot: what they cost is a fact
        // about how much configuration a workspace has, and the headroom is a
        // fraction of a window.
        let overhead = self.request_overhead(session);
        let used = session.estimated_tokens().saturating_add(overhead);
        let _ = ui
            .send(UiEvent::ContextUsed {
                used,
                window: caps.context_length,
            })
            .await;
        if used < budget {
            return summarizing;
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
        // Free and cannot fail, so it runs however the summarizing went: what
        // one round trims is what the next round does not have to hold.
        if session.estimated_tokens().saturating_add(overhead) < budget {
            return summarizing;
        }

        if summarizing == Summarizing::Failed {
            return summarizing;
        }

        let (drop_count, _) =
            split_for_compaction(&session.messages, self.config.keep_recent_messages);
        if drop_count == 0 {
            return summarizing;
        }

        info!(drop_count, "compacting session history");
        let older: Vec<Message> = session.messages[..drop_count].to_vec();
        let summary = match self.summarize(&session.model, older).await {
            Ok(summary) => summary,
            Err(reason) => {
                warn!(%reason, "compaction failed; continuing with full history");
                // Said out loud, and with the provider's own words in it. This
                // used to be a log line nobody reads, so a gateway answering
                // the summarizer with a 404 looked like the context quietly
                // filling up — the one symptom, and none of the cause.
                let _ = ui
                    .send(UiEvent::Error {
                        message: format!(
                            "The earlier conversation could not be summarized, so this turn \
                             carries its full history and may not fit: {reason}. Not tried again \
                             this turn."
                        ),
                    })
                    .await;
                return Summarizing::Failed;
            }
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
        Summarizing::Allowed
    }

    /// What the next request will carry besides its messages.
    ///
    /// Measured wherever a request has already been answered, because the
    /// provider counted the real thing — its own tokenizer, its own envelope,
    /// the tools as it renders them — and no estimate here can do better than
    /// that. Estimated only for the first request of a session, from the two
    /// things that make up nearly all of it.
    fn request_overhead(&self, session: &Session) -> u32 {
        session.measured_overhead().unwrap_or_else(|| {
            let system = crate::session::estimate_tokens(&self.config.system_prompt);
            let tools: u32 = self
                .registry
                .definitions()
                .iter()
                .map(|tool| {
                    crate::session::estimate_tokens(&tool.name)
                        + crate::session::estimate_tokens(&tool.description)
                        + crate::session::estimate_tokens(&tool.input_schema.to_string())
                })
                .sum();
            system.saturating_add(tools)
        })
    }

    /// The older half of the conversation, in one paragraph, or why not.
    ///
    /// The reason comes back rather than being dropped because it is the only
    /// account of a request the user never sees listed: the summarizer's turn
    /// is not in the transcript, so a failure here has no other way to be read.
    async fn summarize(&self, model: &str, messages: Vec<Message>) -> Result<String, String> {
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
        match handle.await {
            Err(e) => return Err(format!("the request could not be run: {e}")),
            Ok(Err(e)) => return Err(e.to_string()),
            Ok(Ok(_)) => {}
        }

        let text = acc.finish().0.text();
        if text.trim().is_empty() {
            return Err("the model returned an empty summary".into());
        }
        Ok(text)
    }
}

/// Whether summarizing the older history is still worth attempting this turn.
///
/// Trimming tool output is free and runs regardless; this governs only the half
/// that costs a request. See the note at its declaration in [`Agent::run`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Summarizing {
    Allowed,
    Failed,
}

/// This round's calls, as name and the answer each one got, if every one of
/// them failed.
///
/// `None` when anything succeeded and when there were no calls at all: both are
/// progress, and neither should count toward a stall. Call ids are deliberately
/// not part of the identity — the model mints a fresh one each time, so two
/// genuinely identical calls never share one.
///
/// # Why the error and not the arguments
///
/// This used to be keyed on the arguments the model sent, and that is the
/// wrong half of the exchange to watch. What the caller wants to know is
/// whether the model has been told this answer before and has learned nothing
/// since — and a model retrying a rejected call rarely sends byte-identical
/// JSON. It reorders a key, rewords a field the tool ignores, adds a space.
/// Every one of those is a different `Value`, so the counter reset on every
/// round and a turn could retry the same refusal until it hit the iteration
/// ceiling.
///
/// The reported case: `update_plan` refused for sending a plan identical to the
/// one on the board, retried with a reworded `active_form` each time — refused
/// identically every round, and counted as a first offence every round.
///
/// Keying on the answer conflates two different calls that failed the same way,
/// which is correct rather than a compromise: the model got the same sentence
/// back both times, and the same sentence twice is the thing being counted.
fn all_failed(assistant: &Message, results: &[ContentBlock]) -> Option<Vec<(String, String)>> {
    let failed: std::collections::HashMap<&str, String> = results
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error: true,
                content,
            } => Some((tool_use_id.as_str(), content.to_text().into_owned())),
            _ => None,
        })
        .collect();

    let mut calls = Vec::new();
    for (id, name, _input) in assistant.tool_uses() {
        let answer = failed.get(id)?;
        calls.push((name.to_string(), answer.clone()));
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
    async fn transcript(&self, session: String, agent: String) {
        let _ = self
            .ui
            .send(UiEvent::ToolTranscript {
                id: self.id.clone(),
                session,
                agent,
            })
            .await;
    }

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
