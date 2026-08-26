//! End-to-end tests for the agent loop against a scripted provider.

use std::sync::Arc;

use taurus_core::testing::{FakeProvider, ScriptedTurn};
use taurus_core::{Agent, AgentConfig, AgentError, Session, TurnRecorder, UiEvent};
use taurus_provider::{ContentBlock, Message, Role, StopReason};
use taurus_tools::{
    AllowAll, DenyAll, PermissionEngine, PermissionPrompt, ToolContext, ToolRegistry,
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Harness {
    agent: Agent,
    provider: Arc<FakeProvider>,
    cancel: CancellationToken,
    _dir: TempDir,
    workspace: std::path::PathBuf,
}

fn harness(turns: Vec<ScriptedTurn>) -> Harness {
    harness_with(turns, Box::new(AllowAll), AgentConfig::default(), 128_000)
}

fn harness_with(
    turns: Vec<ScriptedTurn>,
    prompt: Box<dyn PermissionPrompt>,
    config: AgentConfig,
    context_length: u32,
) -> Harness {
    harness_on(
        FakeProvider::with_context_length(turns, context_length),
        prompt,
        config,
    )
}

/// A harness on a provider the caller built, for the tests that care what the
/// backend can *do* — read images, say — rather than what it answers.
fn harness_on(
    provider: Arc<FakeProvider>,
    prompt: Box<dyn PermissionPrompt>,
    config: AgentConfig,
) -> Harness {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let cancel = CancellationToken::new();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        prompt,
    ));
    let tools = ToolContext::new(workspace.clone(), permissions, cancel.clone());
    let agent = Agent::new(
        provider.clone(),
        ToolRegistry::with_builtins(),
        tools,
        config,
    );
    Harness {
        agent,
        provider,
        cancel,
        _dir: dir,
        workspace,
    }
}

fn recording(h: Harness, recorder: Arc<dyn TurnRecorder>) -> Harness {
    Harness {
        agent: h.agent.with_recorder(recorder),
        ..h
    }
}

/// A recorder that remembers how much of the conversation it was handed, and
/// when. How far a transcript got is the whole question here — a recorder
/// called only at the end would pass any assertion about content.
#[derive(Default)]
struct Spy {
    snapshots: tokio::sync::Mutex<Vec<usize>>,
}

#[async_trait::async_trait]
impl TurnRecorder for Spy {
    async fn record(&self, session: &Session) {
        self.snapshots.lock().await.push(session.messages.len());
    }
}

/// Runs a turn and returns the outcome alongside every UI event emitted.
async fn run(
    h: &Harness,
    session: &mut Session,
    text: &str,
) -> (Result<taurus_core::TurnOutcome, AgentError>, Vec<UiEvent>) {
    let (tx, mut rx) = mpsc::channel(256);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        events
    });
    let outcome = h.agent.run_turn(session, Message::user(text), tx).await;
    (outcome, collector.await.unwrap())
}

#[tokio::test]
async fn a_plain_answer_ends_the_turn_in_one_iteration() {
    let h = harness(vec![ScriptedTurn::text("Hello there")]);
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "hi").await;

    let outcome = outcome.unwrap();
    assert_eq!(outcome.iterations, 1);
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(session.messages.len(), 2, "user + assistant");
    assert_eq!(session.messages[1].text(), "Hello there");
    assert!(events
        .iter()
        .any(|e| matches!(e, UiEvent::TextDelta { text } if text == "Hello there")));
}

/// Every `TextDelta` in order, and how many events carried them.
fn text_deltas(events: &[UiEvent]) -> (String, usize) {
    let parts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            UiEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    (parts.concat(), parts.len())
}

#[tokio::test]
async fn a_run_of_tokens_crosses_to_the_ui_in_one_piece() {
    /*
     * One `UiEvent` per token means one serialize, one webview message and one
     * JS dispatch per token, and a small local model streams hundreds a second
     * — all of them competing with rendering for the same main thread. The
     * gathering window is short enough that a scripted turn, which streams with
     * no pauses at all, arrives inside a single one.
     *
     * The text itself has to survive intact: the frontend appends what it is
     * given, so a delta dropped or reordered here is a word missing from the
     * answer on screen.
     */
    let h = harness(vec![ScriptedTurn::tokens(&[
        "The ", "quick ", "brown ", "fox ", "jumps",
    ])]);
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "hi").await;

    outcome.unwrap();
    let (text, pieces) = text_deltas(&events);
    assert_eq!(text, "The quick brown fox jumps");
    assert!(pieces < 5, "{pieces} events for 5 tokens is not coalescing");
    assert_eq!(session.messages[1].text(), "The quick brown fox jumps");
}

#[tokio::test]
async fn thinking_is_never_gathered_into_the_answer_beside_it() {
    // The two are separate messages on screen. A run of one absorbing the other
    // would put the model's reasoning into what it said out loud.
    let h = harness(vec![ScriptedTurn::thinks_then_says(
        &["let me ", "check"],
        &["Here ", "you ", "go"],
    )]);
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "hi").await;

    outcome.unwrap();
    let (text, _) = text_deltas(&events);
    assert_eq!(text, "Here you go");

    let thinking: String = events
        .iter()
        .filter_map(|e| match e {
            UiEvent::ThinkingDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking, "let me check");

    // And in that order, which the flush on a change of kind is what preserves.
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            UiEvent::ThinkingDelta { .. } => Some("thinking"),
            UiEvent::TextDelta { .. } => Some("text"),
            _ => None,
        })
        .collect();
    let first_text = kinds.iter().position(|k| *k == "text").unwrap();
    assert!(
        !kinds[first_text..].contains(&"thinking"),
        "thinking arrived after the answer began: {kinds:?}"
    );
}

#[tokio::test]
async fn a_stream_that_dies_mid_answer_still_shows_what_it_said() {
    // The gathered deltas are handed on before the failure is looked at. This
    // is also what `produced_output` promises — the loop refuses to retry a
    // stream that already put text on screen, so that text has to be on screen.
    let h = harness(vec![
        ScriptedTurn::transient_failure_after_text("half an ans"),
        ScriptedTurn::text("recovered"),
    ]);
    let mut session = Session::new("fake");
    let (_, events) = run(&h, &mut session, "hi").await;

    let (text, _) = text_deltas(&events);
    assert!(text.starts_with("half an ans"), "{text:?}");
}

#[tokio::test]
async fn a_tool_call_runs_and_the_result_feeds_the_next_iteration() {
    let h = harness(vec![
        ScriptedTurn::tool_call(
            "t1",
            "write_file",
            serde_json::json!({
                "path": "note.txt", "content": "written by the agent"
            }),
        ),
        ScriptedTurn::text("Done."),
    ]);
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "write a note").await;

    let outcome = outcome.unwrap();
    assert_eq!(outcome.iterations, 2);
    assert_eq!(
        std::fs::read_to_string(h.workspace.join("note.txt")).unwrap(),
        "written by the agent"
    );

    // The model must have seen the tool result on its second request.
    let last = h.provider.last_request().await.unwrap();
    let saw_result = last.messages.iter().any(|m| {
        m.content
            .iter()
            .any(|b| matches!(b, taurus_provider::ContentBlock::ToolResult { .. }))
    });
    assert!(saw_result, "second request omitted the tool result");

    assert!(events
        .iter()
        .any(|e| matches!(e, UiEvent::ToolCallFinished { ok: true, .. })));
}

#[tokio::test]
async fn a_failing_tool_reports_back_to_the_model_instead_of_aborting() {
    let h = harness(vec![
        ScriptedTurn::tool_call(
            "t1",
            "read_file",
            serde_json::json!({"path": "missing.txt"}),
        ),
        ScriptedTurn::text("I could not read it."),
    ]);
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "read missing.txt").await;

    assert!(outcome.is_ok(), "a tool failure must not fail the turn");
    assert!(events
        .iter()
        .any(|e| matches!(e, UiEvent::ToolCallFinished { ok: false, .. })));

    let error_result = session.messages.iter().any(|m| {
        m.content.iter().any(|b| {
            matches!(
                b,
                taurus_provider::ContentBlock::ToolResult { is_error: true, .. }
            )
        })
    });
    assert!(error_result, "the error was not recorded as a tool result");
}

#[tokio::test]
async fn a_denied_tool_tells_the_model_not_to_retry() {
    let h = harness_with(
        vec![
            ScriptedTurn::tool_call(
                "t1",
                "write_file",
                serde_json::json!({"path": "x.txt", "content": "no"}),
            ),
            ScriptedTurn::text("Understood."),
        ],
        Box::new(DenyAll),
        AgentConfig::default(),
        128_000,
    );
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "write x").await;

    assert!(outcome.is_ok());
    assert!(
        !h.workspace.join("x.txt").exists(),
        "denied write touched the disk"
    );

    let told = session.messages.iter().any(|m| {
        m.content.iter().any(|b| match b {
            taurus_provider::ContentBlock::ToolResult { content, .. } => {
                content.to_text().contains("denied") && content.to_text().contains("Do not retry")
            }
            _ => false,
        })
    });
    assert!(told, "the model was not told the action was denied");
}

#[tokio::test]
async fn several_tool_calls_return_results_in_the_order_they_were_requested() {
    let h = harness(vec![
        ScriptedTurn::tool_calls(vec![
            ("a", "list_dir", serde_json::json!({})),
            ("b", "glob", serde_json::json!({"pattern": "**/*.none"})),
            ("c", "list_dir", serde_json::json!({})),
        ]),
        ScriptedTurn::text("ok"),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "look around").await;
    assert!(outcome.is_ok());

    let results = session
        .messages
        .iter()
        .find(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, taurus_provider::ContentBlock::ToolResult { .. }))
        })
        .expect("no tool results recorded");

    let ids: Vec<&str> = results
        .content
        .iter()
        .filter_map(|b| match b {
            taurus_provider::ContentBlock::ToolResult { tool_use_id, .. } => {
                Some(tool_use_id.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["a", "b", "c"], "results were reordered");
}

#[tokio::test]
async fn an_unparseable_prompted_call_gets_syntax_guidance_not_a_missing_tool_error() {
    let h = harness(vec![
        ScriptedTurn::tool_call(
            "t1",
            taurus_provider::prompted::MALFORMED_TOOL,
            serde_json::json!("garbage"),
        ),
        ScriptedTurn::text("Retrying."),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "do something").await;
    assert!(outcome.is_ok());

    let guidance = session.messages.iter().any(|m| {
        m.content.iter().any(|b| match b {
            taurus_provider::ContentBlock::ToolResult { content, .. } => {
                content.to_text().contains("<tool_call>")
            }
            _ => false,
        })
    });
    assert!(
        guidance,
        "the model got no guidance on how to fix its syntax"
    );
}

#[tokio::test]
async fn the_iteration_ceiling_stops_a_model_stuck_in_a_tool_loop() {
    // Every scripted turn calls a tool, so only the ceiling can end this.
    let turns = (0..30)
        .map(|_| ScriptedTurn::tool_call("t", "list_dir", serde_json::json!({})))
        .collect();
    let config = AgentConfig {
        max_iterations: 4,
        ..Default::default()
    };
    let h = harness_with(turns, Box::new(AllowAll), config, 128_000);
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "spin").await;

    assert!(matches!(outcome, Err(AgentError::IterationLimit(4))));
    assert_eq!(h.provider.request_count().await, 4);
    assert!(events.iter().any(|e| matches!(e, UiEvent::Error { .. })));
}

#[tokio::test]
async fn cancellation_before_a_turn_starts_returns_immediately() {
    let h = harness(vec![ScriptedTurn::text("should not be reached")]);
    h.cancel.cancel();
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "hi").await;

    let outcome = outcome.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Canceled);
    assert_eq!(h.provider.request_count().await, 0);
}

#[tokio::test]
async fn cancellation_partway_through_stops_the_loop_without_a_further_request() {
    // Cancel fires as the provider serves the second request, so the loop must
    // stop there rather than continuing to a third.
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let cancel = CancellationToken::new();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));
    let tools = ToolContext::new(workspace, permissions, cancel.clone());
    let provider = FakeProvider::cancelling_after(
        vec![
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({})),
            ScriptedTurn::tool_call("t2", "list_dir", serde_json::json!({})),
            ScriptedTurn::text("unreached"),
        ],
        2,
    );
    let agent = Agent::new(
        provider.clone(),
        ToolRegistry::with_builtins(),
        tools,
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let outcome = agent
        .run_turn(&mut session, Message::user("go"), tx)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Canceled);
    assert_eq!(
        provider.request_count().await,
        2,
        "the loop kept going after cancellation"
    );
}

#[tokio::test]
async fn superseded_tool_output_is_trimmed_instead_of_summarized() {
    let h = harness_with(
        // One turn only. A second would mean the summarizer ran.
        vec![ScriptedTurn::text("Carrying on.")],
        Box::new(AllowAll),
        AgentConfig {
            keep_recent_messages: 2,
            compaction_threshold: 0.8,
            ..Default::default()
        },
        1000,
    );

    // The same file read four times over. Only the last answer is current, so
    // three bulky results are dead weight the trim pass can collapse.
    let mut session = Session::new("fake");
    for i in 0..4 {
        session.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                format!("t{i}"),
                "read_file",
                serde_json::json!({ "path": "a.rs" }),
            )],
        ));
        session.push(Message::new(
            Role::User,
            vec![ContentBlock::tool_result(
                format!("t{i}"),
                "fn main() {}\n".repeat(160),
            )],
        ));
    }
    let before = session.messages.len();

    let (outcome, events) = run(&h, &mut session, "continue").await;
    assert!(outcome.is_ok());

    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::ContextTrimmed { .. })),
        "the trim pass did not run: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, UiEvent::Compacted { .. })),
        "trimming got under budget, so nothing should have been summarized"
    );
    assert_eq!(
        h.provider.request_count().await,
        1,
        "a summarizer request was made when trimming was enough"
    );

    // Trimming shortens messages rather than removing them, which is what keeps
    // every tool call paired with a result.
    assert_eq!(session.messages.len(), before + 2);
    let trimmed = match &session.messages[1].content[0] {
        ContentBlock::ToolResult { content, .. } => content.clone(),
        other => panic!("expected a tool result, got {other:?}"),
    };
    let trimmed = trimmed.to_text();
    assert!(trimmed.contains("called again later"), "{trimmed}");
}

#[tokio::test]
async fn a_summarizer_that_fails_is_not_asked_again_this_turn() {
    // The session stays over budget for the whole turn, so every iteration
    // reaches the compaction check. One broken summarizer request used to
    // become one per iteration — up to `max_iterations` of them — and with the
    // reason swallowed, a gateway answering with a 404 looked from the outside
    // like the context quietly filling up.
    let h = harness_with(
        vec![
            // Consumed by the summarizer, which fails.
            ScriptedTurn::permanent_failure(),
            // The turn carries on: a tool call, then an answer. The second
            // iteration reaches the compaction check with the session still
            // over budget.
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({})),
            ScriptedTurn::text("Carrying on."),
        ],
        Box::new(AllowAll),
        AgentConfig {
            keep_recent_messages: 2,
            compaction_threshold: 0.8,
            ..Default::default()
        },
        1000,
    );

    let mut session = Session::new("fake");
    for i in 0..20 {
        session.push(Message::user(format!(
            "old message {i} {}",
            "x".repeat(300)
        )));
    }

    let (outcome, events) = run(&h, &mut session, "continue").await;
    assert!(outcome.is_ok(), "{outcome:?}");

    let summarizer_requests = h
        .provider
        .seen
        .lock()
        .await
        .iter()
        .filter(|r| {
            r.system
                .as_deref()
                .is_some_and(|s| s.starts_with("Summarize the conversation"))
        })
        .count();
    assert_eq!(
        summarizer_requests, 1,
        "the summarizer was asked again after failing"
    );

    // And the reason reaches the reader, once, in the provider's own words.
    let said: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Error { message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("could not be summarized"), "{}", said[0]);
    assert!(said[0].contains("invalid api key"), "{}", said[0]);

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, UiEvent::Compacted { .. })),
        "nothing was summarized, so nothing was compacted"
    );
}

#[tokio::test]
async fn history_is_compacted_when_it_outgrows_the_context_window() {
    let h = harness_with(
        vec![
            // First request triggers compaction; the summarizer answers, then
            // the real turn runs.
            ScriptedTurn::text("SUMMARY OF EARLIER WORK"),
            ScriptedTurn::text("Carrying on."),
        ],
        Box::new(AllowAll),
        AgentConfig {
            keep_recent_messages: 2,
            compaction_threshold: 0.8,
            ..Default::default()
        },
        1000,
    );

    let mut session = Session::new("fake");
    for i in 0..20 {
        session.push(Message::user(format!(
            "old message {i} {}",
            "x".repeat(300)
        )));
    }
    let before = session.messages.len();

    let (outcome, events) = run(&h, &mut session, "continue").await;
    assert!(outcome.is_ok());

    assert!(
        session.messages.len() < before,
        "history was not compacted: {} -> {}",
        before,
        session.messages.len()
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, UiEvent::Compacted { .. })));
    assert!(
        session.messages[0]
            .text()
            .contains("SUMMARY OF EARLIER WORK"),
        "the summary did not replace the dropped history"
    );
}

#[tokio::test]
async fn the_system_prompt_and_tool_definitions_reach_the_provider() {
    let config = AgentConfig {
        system_prompt: "You are Taurus.".into(),
        ..Default::default()
    };
    let h = harness_with(
        vec![ScriptedTurn::text("ok")],
        Box::new(AllowAll),
        config,
        128_000,
    );
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "hi").await;
    assert!(outcome.is_ok());

    let request = h.provider.last_request().await.unwrap();
    assert_eq!(request.system.as_deref(), Some("You are Taurus."));
    assert!(request.tools.iter().any(|t| t.name == "read_file"));
    assert!(request.tools.iter().any(|t| t.name == "run_command"));
}

#[tokio::test]
async fn allowed_tools_restricts_what_the_model_is_offered() {
    let config = AgentConfig {
        allowed_tools: vec!["read_file".into(), "glob".into()],
        ..Default::default()
    };
    let h = harness_with(
        vec![ScriptedTurn::text("ok")],
        Box::new(AllowAll),
        config,
        128_000,
    );
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "hi").await;
    assert!(outcome.is_ok());

    let names: Vec<String> = h
        .provider
        .last_request()
        .await
        .unwrap()
        .tools
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, vec!["glob", "read_file"]);
}

// ---------------------------------------------------------------------------
// Transient failures
//
// `ProviderError::is_transient` classifies a 429 or a 5xx as worth another go.
// These cover what the loop does with that answer — including the case where it
// must ignore it.

/// A config that retries without ever sleeping. The backoff is real behaviour
/// but its duration is not what any of these assert on.
fn instant_retries(retries: u32) -> AgentConfig {
    AgentConfig {
        max_transient_retries: retries,
        retry_backoff: std::time::Duration::ZERO,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_transient_failure_is_retried_and_the_turn_survives() {
    let h = harness_with(
        vec![
            ScriptedTurn::transient_failure(),
            ScriptedTurn::text("Recovered."),
        ],
        Box::new(AllowAll),
        instant_retries(3),
        128_000,
    );
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "hi").await;

    assert!(
        outcome.is_ok(),
        "a 503 should not end the turn: {outcome:?}"
    );
    assert_eq!(session.messages[1].text(), "Recovered.");
    assert_eq!(
        h.provider.request_count().await,
        2,
        "should have retried once"
    );

    let retried = events.iter().find_map(|e| match e {
        UiEvent::Retrying { attempt, of, .. } => Some((*attempt, *of)),
        _ => None,
    });
    assert_eq!(
        retried,
        Some((2, 4)),
        "the user must be told a retry is why they are waiting"
    );
}

#[tokio::test]
async fn a_failure_part_way_through_an_answer_is_not_retried() {
    // The half-answer is already on screen. Retrying would write it twice.
    let h = harness_with(
        vec![
            ScriptedTurn::transient_failure_after_text("The answer is "),
            ScriptedTurn::text("42."),
        ],
        Box::new(AllowAll),
        instant_retries(3),
        128_000,
    );
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "hi").await;

    assert!(
        matches!(outcome, Err(AgentError::Provider(_))),
        "a stream that died mid-answer must surface, not retry: {outcome:?}"
    );
    assert_eq!(h.provider.request_count().await, 1);
    assert!(
        !events.iter().any(|e| matches!(e, UiEvent::Retrying { .. })),
        "nothing should have been retried"
    );
}

#[tokio::test]
async fn a_permanent_failure_is_not_retried() {
    let h = harness_with(
        vec![ScriptedTurn::permanent_failure(), ScriptedTurn::text("hi")],
        Box::new(AllowAll),
        instant_retries(3),
        128_000,
    );
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "hi").await;

    assert!(matches!(outcome, Err(AgentError::Provider(_))));
    assert_eq!(
        h.provider.request_count().await,
        1,
        "a bad API key does not improve on the second attempt"
    );
}

#[tokio::test]
async fn retries_are_bounded() {
    let h = harness_with(
        (0..6).map(|_| ScriptedTurn::transient_failure()).collect(),
        Box::new(AllowAll),
        instant_retries(2),
        128_000,
    );
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "hi").await;

    assert!(matches!(outcome, Err(AgentError::Provider(_))));
    assert_eq!(
        h.provider.request_count().await,
        3,
        "one attempt plus two retries"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, UiEvent::Retrying { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn canceling_during_a_backoff_does_not_wait_it_out() {
    // A backoff long enough that sitting through it would fail the timeout
    // below, so only honoring the cancellation can pass this.
    let config = AgentConfig {
        max_transient_retries: 3,
        retry_backoff: std::time::Duration::from_secs(30),
        ..Default::default()
    };
    let h = harness_with(
        (0..4).map(|_| ScriptedTurn::transient_failure()).collect(),
        Box::new(AllowAll),
        config,
        128_000,
    );
    let cancel = h.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
    });

    let mut session = Session::new("fake");
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run(&h, &mut session, "hi"),
    )
    .await;
    assert!(
        outcome.is_ok(),
        "cancellation should cut the backoff short rather than run it to term"
    );
}

// ---------------------------------------------------------------------------
// Stalls
//
// The system prompt tells the model not to retry a failed call unchanged. These
// are what make that true rather than merely stated.

/// Calls `read_file` on a path that is not there, which fails every time.
fn failing_read(id: &str, path: &str) -> ScriptedTurn {
    ScriptedTurn::tool_call(id, "read_file", serde_json::json!({ "path": path }))
}

#[tokio::test]
async fn repeating_one_failing_call_unchanged_stops_the_turn() {
    let h = harness(vec![
        failing_read("t1", "missing.txt"),
        failing_read("t2", "missing.txt"),
        failing_read("t3", "missing.txt"),
        ScriptedTurn::text("never reached"),
    ]);
    let mut session = Session::new("fake");
    let (outcome, events) = run(&h, &mut session, "read it").await;

    assert!(
        matches!(outcome, Err(AgentError::Stalled(3))),
        "expected a stall after three identical failures: {outcome:?}"
    );
    assert_eq!(
        h.provider.request_count().await,
        3,
        "the turn should stop rather than spend its whole iteration budget"
    );
    assert!(
        events.iter().any(|e| matches!(e, UiEvent::Error { .. })),
        "the user must be told why the turn stopped"
    );
    // The transcript has to carry the reason, or a resumed session finds a turn
    // that simply ends.
    assert!(session
        .messages
        .last()
        .unwrap()
        .text()
        .contains("failed 3 times"));
}

#[tokio::test]
async fn a_failing_call_that_changes_is_not_a_stall() {
    // Three failures, but the model is trying something new each time. That is
    // the model working, not the model stuck.
    let h = harness(vec![
        failing_read("t1", "one.txt"),
        failing_read("t2", "two.txt"),
        failing_read("t3", "three.txt"),
        ScriptedTurn::text("None of those exist."),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "find it").await;

    assert!(
        outcome.is_ok(),
        "changing the argument is progress: {outcome:?}"
    );
    assert_eq!(
        session.messages.last().unwrap().text(),
        "None of those exist."
    );
}

#[tokio::test]
async fn alternating_between_two_failing_calls_stops_the_turn() {
    // A model going A, B, A, B, A is as stuck as one going A, A, A — but every
    // round differs from the round before it, so comparing against that one
    // alone saw progress and let the whole iteration budget drain.
    let h = harness(vec![
        failing_read("t1", "one.txt"),
        failing_read("t2", "two.txt"),
        failing_read("t3", "one.txt"),
        failing_read("t4", "two.txt"),
        failing_read("t5", "one.txt"),
        ScriptedTurn::text("never reached"),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "read it").await;

    assert!(
        matches!(outcome, Err(AgentError::Stalled(3))),
        "expected a stall on the third failure of the same call: {outcome:?}"
    );
    assert_eq!(
        h.provider.request_count().await,
        5,
        "the turn should stop on the third `one.txt`, not run to the ceiling"
    );
}

/// A harness with `update_plan` wired the way a real turn wires it: the tool in
/// the registry, the board on the agent, both sharing one `PlanBoard`.
fn plan_harness(turns: Vec<ScriptedTurn>) -> Harness {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let cancel = CancellationToken::new();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));
    let tools = ToolContext::new(workspace.clone(), permissions, cancel.clone());
    let provider = FakeProvider::with_context_length(turns, 128_000);

    let board = taurus_tools::PlanBoard::new();
    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(taurus_tools::builtin::plan::UpdatePlan::new(
        board.clone(),
    )));

    Harness {
        agent: Agent::new(provider.clone(), registry, tools, AgentConfig::default())
            .with_plan(board),
        provider,
        cancel,
        _dir: dir,
        workspace,
    }
}

fn set_plan(id: &str, active_form: &str) -> ScriptedTurn {
    ScriptedTurn::tool_call(
        id,
        "update_plan",
        serde_json::json!({ "steps": [
            { "text": "Verify the project setup", "state": "active", "active_form": active_form },
            { "text": "Execute implementation phase-by-phase", "state": "todo" },
        ] }),
    )
}

#[tokio::test]
async fn retrying_a_refusal_with_reworded_arguments_still_stops_the_turn() {
    /*
     * The reported loop, end to end.
     *
     * `update_plan` refuses a plan identical to the one already on the board,
     * and a model that reads the refusal as "try again" sends it back with the
     * one field the tool ignores reworded. Nothing about the exchange has
     * moved — the same refusal, four times — but every round carried different
     * JSON, and while the stall detector compared arguments that was four
     * first offences in a row. The turn ran to its iteration ceiling.
     *
     * Counting the *answer* instead is what makes this stop: the model has
     * been told the same sentence three times and has learned nothing from it.
     */
    let h = plan_harness(vec![
        set_plan("t1", "Verifying the project setup"),
        set_plan("t2", "Working on the project setup"),
        set_plan("t3", "Checking the project setup"),
        set_plan("t4", "Looking at the project setup"),
        ScriptedTurn::text("never reached"),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "do the thing").await;

    assert!(
        matches!(outcome, Err(AgentError::Stalled(3))),
        "the same refusal three times is a stall however the arguments read: {outcome:?}"
    );
    assert_eq!(
        h.provider.request_count().await,
        4,
        "the first call sets the plan; the turn stops on the third refusal"
    );
}

#[tokio::test]
async fn a_refused_call_is_not_told_to_retry() {
    /*
     * The other half of the same bug, and the more embarrassing one: the
     * refusal was an `InvalidInput`, and every `InvalidInput` had "Check the
     * tool's schema and retry" appended to it. So the message ended by telling
     * the model to do the thing it had just been refused for, and the models
     * did.
     */
    let h = plan_harness(vec![
        set_plan("t1", "Verifying the project setup"),
        set_plan("t2", "Verifying the project setup"),
        ScriptedTurn::text("Right — doing step 1 now."),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "do the thing").await;
    assert!(
        outcome.is_ok(),
        "two rounds is not yet a stall: {outcome:?}"
    );

    let refusal = session
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .find_map(|b| match b {
            ContentBlock::ToolResult {
                is_error: true,
                content,
                ..
            } => Some(content.to_text().into_owned()),
            _ => None,
        })
        .expect("the second call is refused");

    assert!(refusal.contains("already on the board"), "{refusal}");
    assert!(
        !refusal.contains("retry"),
        "a refusal must not end by asking for the call again: {refusal}"
    );
}

#[tokio::test]
async fn anything_succeeding_clears_the_count() {
    // The guard against the widened check ending turns it should not. This
    // fails on `missing.txt` four times in all, which would trip a counter that
    // simply accumulated — but something worked in between, and a model that is
    // getting somewhere is allowed to come back to a call that failed earlier.
    let h = harness(vec![
        failing_read("t1", "missing.txt"),
        failing_read("t2", "missing.txt"),
        ScriptedTurn::tool_call("t3", "list_dir", serde_json::json!({ "path": "." })),
        failing_read("t4", "missing.txt"),
        failing_read("t5", "missing.txt"),
        ScriptedTurn::text("It really is not there."),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "find it").await;

    assert!(
        outcome.is_ok(),
        "progress in between must reset the count: {outcome:?}"
    );
    assert_eq!(
        session.messages.last().unwrap().text(),
        "It really is not there."
    );
}

#[tokio::test]
async fn repeating_a_call_that_succeeds_is_not_a_stall() {
    // A model may legitimately repeat a call while it works — polling a build,
    // re-listing a directory it is changing. Only a repeat that keeps failing
    // is a stall.
    let h = harness(vec![
        ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({ "path": "." })),
        ScriptedTurn::tool_call("t2", "list_dir", serde_json::json!({ "path": "." })),
        ScriptedTurn::tool_call("t3", "list_dir", serde_json::json!({ "path": "." })),
        ScriptedTurn::text("Still empty."),
    ]);
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "watch it").await;

    assert!(
        outcome.is_ok(),
        "a succeeding repeat is not a stall: {outcome:?}"
    );
}

#[tokio::test]
async fn a_command_the_model_ran_can_be_undone() {
    // The gap this closes, through the whole loop rather than the registry
    // alone: no tool declared these paths, the model reached them with a shell
    // command, and the turn is still recoverable afterwards.
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    std::fs::write(workspace.join("keep.txt"), "the user's work").unwrap();
    std::fs::write(workspace.join("doomed.txt"), "also the user's work").unwrap();

    let logs = TempDir::new().unwrap();
    let store = taurus_tools::CheckpointStore::new(logs.path());
    let cancel = CancellationToken::new();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));
    let tools = ToolContext::new(workspace.clone(), permissions, cancel)
        .with_checkpoints(store.begin_turn("s1", &workspace, "tidy up"));

    // What a model actually does: one command, several files, none declared.
    let command = if cfg!(windows) {
        "echo clobbered > keep.txt & del doomed.txt & echo new > built.txt"
    } else {
        "echo clobbered > keep.txt; rm doomed.txt; echo new > built.txt"
    };
    let provider = FakeProvider::new(vec![
        ScriptedTurn::tool_call("t1", "run_command", serde_json::json!({"command": command})),
        ScriptedTurn::text("Tidied."),
    ]);
    let agent = Agent::new(
        provider,
        ToolRegistry::with_builtins(),
        tools,
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        events
    });
    agent
        .run_turn(&mut session, Message::user("tidy up"), tx)
        .await
        .unwrap();
    let events = collector.await.unwrap();

    assert!(
        !workspace.join("doomed.txt").exists(),
        "the command no-opped"
    );

    // It reaches the list the app reads for its changed-file count and its
    // Changes drawer, which is where the user is told.
    let turns = store.turns("s1").unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].files, vec!["built.txt", "doomed.txt", "keep.txt"]);

    // And the turn is reported as ordinary. A sweep that covered the command
    // has nothing to add to the result; only one that could not covers says so.
    let finished = events.iter().find_map(|e| match e {
        UiEvent::ToolCallFinished { output, .. } => Some(output),
        _ => None,
    });
    assert!(
        !finished.unwrap().contains("[taurus]"),
        "a covered command should carry no warning: {finished:?}"
    );

    store.rewind("s1", &workspace, 1, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
        "the user's work"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("doomed.txt")).unwrap(),
        "also the user's work",
        "a deleted file is the case nothing else in the harness remembers"
    );
    assert!(
        !workspace.join("built.txt").exists(),
        "a file the command created must not survive the rewind"
    );
}

/// A harness whose turns are checkpointed, which is what the app and the CLI
/// both do — and what the verify nudge keys off, since the checkpoint log is
/// the authority on whether a turn actually changed anything.
fn recorded(
    turns: Vec<ScriptedTurn>,
    config: AgentConfig,
) -> (
    Agent,
    Arc<FakeProvider>,
    std::path::PathBuf,
    TempDir,
    TempDir,
) {
    let dir = TempDir::new().unwrap();
    let logs = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));
    let store = taurus_tools::CheckpointStore::new(logs.path());
    let tools = ToolContext::new(workspace.clone(), permissions, CancellationToken::new())
        .with_checkpoints(store.begin_turn("s1", &workspace, "do some work"));
    let provider = FakeProvider::new(turns);
    let agent = Agent::new(
        provider.clone(),
        ToolRegistry::with_builtins(),
        tools,
        config,
    );
    (agent, provider, workspace, dir, logs)
}

async fn drive(agent: &Agent, session: &mut Session, text: &str) {
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let _ = agent.run_turn(session, Message::user(text), tx).await;
}

/// How many times the nudge appears in what the provider was last sent.
fn nudges(request: &taurus_provider::ChatRequest) -> usize {
    request
        .messages
        .iter()
        .filter(|m| {
            m.content.iter().any(|b| {
                matches!(b, ContentBlock::Text { text } if text.contains("have not run anything since"))
            })
        })
        .count()
}

/// Everything a turn sent, kept in order.
async fn collect(agent: &Agent, session: &mut Session, text: &str) -> Vec<UiEvent> {
    let (tx, mut rx) = mpsc::channel(256);
    let sink = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(event) = rx.recv().await {
            seen.push(event);
        }
        seen
    });
    let _ = agent.run_turn(session, Message::user(text), tx).await;
    sink.await.unwrap()
}

/// Every block of every message the provider was actually sent.
fn sent_blocks(request: &taurus_provider::ChatRequest) -> Vec<&ContentBlock> {
    request.messages.iter().flat_map(|m| &m.content).collect()
}

#[tokio::test]
async fn a_model_that_cannot_see_is_told_an_image_was_there() {
    // What makes moving a conversation to a text-only model survivable. The
    // picture stays in the session and in the transcript; only the request is
    // rewritten, so moving back to a model that can see brings it back.
    let provider = FakeProvider::new(vec![ScriptedTurn::text("I cannot see it.")]);
    let h = harness_on(provider.clone(), Box::new(AllowAll), AgentConfig::default());

    let mut session = Session::new("fake");
    session.push(Message::new(
        Role::User,
        vec![
            ContentBlock::Image {
                mime_type: "image/png".into(),
                data: "aGVsbG8=".into(),
            },
            ContentBlock::text("what is wrong with this?"),
        ],
    ));
    let (outcome, _) = run(&h, &mut session, "and now?").await;
    assert!(outcome.is_ok());

    let sent = provider.last_request().await.unwrap();
    let blocks = sent_blocks(&sent);
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        "an image reached a model that cannot read one: {blocks:#?}"
    );
    // Replaced, not dropped: the question beside it says "this", and a question
    // about nothing is worse than one that says what is missing.
    assert!(
        blocks.iter().any(|b| matches!(
            b,
            ContentBlock::Text { text } if text.contains("cannot read images")
        )),
        "{blocks:#?}"
    );
    // And the session itself is untouched.
    assert!(session.messages[0]
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. })));
}

#[tokio::test]
async fn a_model_that_can_see_is_sent_the_picture() {
    let provider = FakeProvider::seeing(vec![ScriptedTurn::text("It is upside down.")]);
    let h = harness_on(provider.clone(), Box::new(AllowAll), AgentConfig::default());

    let mut session = Session::new("fake");
    session.push(Message::new(
        Role::User,
        vec![
            ContentBlock::Image {
                mime_type: "image/png".into(),
                data: "aGVsbG8=".into(),
            },
            ContentBlock::text("what is wrong with this?"),
        ],
    ));
    let (outcome, _) = run(&h, &mut session, "and now?").await;
    assert!(outcome.is_ok());

    let sent = provider.last_request().await.unwrap();
    assert!(
        sent_blocks(&sent)
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        "the picture was withheld from a model that reads them"
    );
}

#[tokio::test]
async fn a_turn_reports_the_files_it_changes_while_it_changes_them() {
    // The count in the header used to be read off the checkpoint log after the
    // turn was over, so a turn spent rewriting the project said "no file
    // changes" for all of it and told the truth once there was nothing left to
    // watch.
    let (agent, _provider, _workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call(
                "t1",
                "write_file",
                serde_json::json!({"path": "a.rs", "content": "fn main() {}"}),
            ),
            ScriptedTurn::tool_call(
                "t2",
                "write_file",
                serde_json::json!({"path": "b.rs", "content": "fn other() {}"}),
            ),
            ScriptedTurn::text("Both written."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    let events = collect(&agent, &mut session, "write two files").await;

    let reports: Vec<&Vec<String>> = events
        .iter()
        .filter_map(|e| match e {
            UiEvent::FilesChanged { paths } => Some(paths),
            _ => None,
        })
        .collect();

    // One per round that changed something, never one for the round that only
    // spoke — and each carries the whole set, so a listener that missed one is
    // not short a file for the rest of the turn.
    assert_eq!(reports.len(), 2, "{events:#?}");
    assert_eq!(reports[0], &vec!["a.rs".to_string()]);
    assert_eq!(reports[1], &vec!["a.rs".to_string(), "b.rs".to_string()]);
}

#[tokio::test]
async fn a_turn_that_changes_nothing_reports_nothing() {
    let (agent, _provider, _workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call("t1", "run_command", serde_json::json!({"command": "true"})),
            ScriptedTurn::text("Nothing to change."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    let events = collect(&agent, &mut session, "have a look").await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, UiEvent::FilesChanged { .. })),
        "{events:#?}"
    );
}

#[tokio::test]
async fn a_turn_that_changed_files_without_checking_is_asked_to_check() {
    // The system prompt already says to run the tests. A small model edits a
    // file and stops anyway, so being told once more, at the moment it tries to
    // finish, is what actually makes it happen.
    let (agent, provider, workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call(
                "t1",
                "write_file",
                serde_json::json!({"path": "a.rs", "content": "fn main() {}"}),
            ),
            ScriptedTurn::text("Done."),
            ScriptedTurn::tool_call("t2", "run_command", serde_json::json!({"command": "true"})),
            ScriptedTurn::text("Checked; it builds."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    drive(&agent, &mut session, "write a.rs").await;

    assert!(workspace.join("a.rs").exists());
    let last = provider.last_request().await.unwrap();
    assert_eq!(nudges(&last), 1, "the model was never asked to check");
    // And it kept going rather than the turn ending on the nudge.
    assert_eq!(provider.request_count().await, 4);
}

#[tokio::test]
async fn a_turn_that_already_checked_its_work_is_left_alone() {
    // Ran a command after editing and it changed nothing — that is the model
    // asking the project a question and getting an answer, which is the whole
    // behavior being asked for. Nagging here would be nagging for compliance.
    let (agent, provider, _workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call(
                "t1",
                "write_file",
                serde_json::json!({"path": "a.rs", "content": "fn main() {}"}),
            ),
            ScriptedTurn::tool_call("t2", "run_command", serde_json::json!({"command": "true"})),
            ScriptedTurn::text("Done, and it builds."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    drive(&agent, &mut session, "write a.rs").await;

    assert_eq!(
        provider.request_count().await,
        3,
        "an unnecessary round trip"
    );
    assert_eq!(nudges(&provider.last_request().await.unwrap()), 0);
}

#[tokio::test]
async fn a_check_that_leaves_a_file_behind_is_still_a_check() {
    // The bug this replaced. A test runner writes a `.coverage` beside the
    // code, and the sweep looks past a file an ignore rule excludes on purpose
    // — so the run registered as work, the debt stood, and the model was told
    // it had not run anything since, one line after running the tests.
    let (agent, provider, workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call(
                "t1",
                "write_file",
                serde_json::json!({"path": "a.rs", "content": "fn main() {}"}),
            ),
            ScriptedTurn::tool_call(
                "t2",
                "run_command",
                serde_json::json!({"command": "echo data > .coverage"}),
            ),
            ScriptedTurn::text("Done, and the tests pass."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    drive(&agent, &mut session, "write a.rs and test it").await;

    assert!(workspace.join(".coverage").exists(), "the check wrote it");
    assert_eq!(
        nudges(&provider.last_request().await.unwrap()),
        0,
        "told to check work it had just checked"
    );
    assert_eq!(provider.request_count().await, 3, "a wasted round trip");
}

#[tokio::test]
async fn writing_after_the_check_still_owes_a_check() {
    // The order is what decides, not which of the two happened. A round that
    // ran the tests and *then* edited has changed something nothing has been
    // run against, exactly as if the edit had come in a round of its own.
    let (agent, provider, _workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_calls(vec![
                ("t1", "run_command", serde_json::json!({"command": "true"})),
                (
                    "t2",
                    "write_file",
                    serde_json::json!({"path": "a.rs", "content": "fn main() {}"}),
                ),
            ]),
            ScriptedTurn::text("Done."),
            ScriptedTurn::tool_call("t3", "run_command", serde_json::json!({"command": "true"})),
            ScriptedTurn::text("Checked."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    drive(&agent, &mut session, "test then write").await;

    assert_eq!(
        nudges(&provider.last_request().await.unwrap()),
        1,
        "an edit after the check went unchecked"
    );
}

#[tokio::test]
async fn a_turn_that_only_read_things_is_left_alone() {
    let (agent, provider, _workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({})),
            ScriptedTurn::text("Empty."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    drive(&agent, &mut session, "look around").await;

    assert_eq!(provider.request_count().await, 2);
    assert_eq!(nudges(&provider.last_request().await.unwrap()), 0);
}

#[tokio::test]
async fn the_model_is_asked_to_check_at_most_once_a_turn() {
    // A model that answers the nudge with prose rather than a command must not
    // be asked again. One round trip is a fair price for the behavior; an
    // argument the model cannot win is not.
    let (agent, provider, _workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call(
                "t1",
                "write_file",
                serde_json::json!({"path": "a.rs", "content": "fn main() {}"}),
            ),
            ScriptedTurn::text("Done."),
            ScriptedTurn::text("Really done."),
            ScriptedTurn::text("Still done."),
        ],
        AgentConfig::default(),
    );

    let mut session = Session::new("fake");
    drive(&agent, &mut session, "write a.rs").await;

    assert_eq!(provider.request_count().await, 3, "it kept arguing");
    assert_eq!(nudges(&provider.last_request().await.unwrap()), 1);
}

#[tokio::test]
async fn the_check_can_be_turned_off() {
    let (agent, provider, _workspace, _dir, _logs) = recorded(
        vec![
            ScriptedTurn::tool_call(
                "t1",
                "write_file",
                serde_json::json!({"path": "a.rs", "content": "fn main() {}"}),
            ),
            ScriptedTurn::text("Done."),
        ],
        AgentConfig {
            verify_changes: false,
            ..AgentConfig::default()
        },
    );

    let mut session = Session::new("fake");
    drive(&agent, &mut session, "write a.rs").await;

    assert_eq!(provider.request_count().await, 2);
    assert_eq!(nudges(&provider.last_request().await.unwrap()), 0);
}

// ------------------------------------------------------------------ the plan

/// A harness whose registry carries `update_plan`, wired to the same board the
/// agent restates from — which is what `Host::build_agent` does for a real turn.
fn planning(turns: Vec<ScriptedTurn>) -> (Agent, Arc<FakeProvider>, TempDir) {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let cancel = CancellationToken::new();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));

    let board = taurus_tools::PlanBoard::new();
    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(taurus_tools::builtin::plan::UpdatePlan::new(
        board.clone(),
    )));

    let provider = FakeProvider::new(turns);
    let agent = Agent::new(
        provider.clone(),
        registry,
        ToolContext::new(workspace, permissions, cancel),
        AgentConfig::default(),
    )
    .with_plan(board);
    (agent, provider, dir)
}

fn plan_call(id: &str, steps: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::tool_call(id, "update_plan", serde_json::json!({ "steps": steps }))
}

/// How many times the model has been asked to close its plan.
/// The plan as the request actually carries it, wherever that is.
///
/// A helper rather than a field read, because *where* is the thing that changed
/// and the properties below are about the plan being in front of the model at
/// all — one copy, always live — not about which field holds it.
fn plan_as_sent(request: &taurus_provider::ChatRequest) -> String {
    let mut out = request.system.clone().unwrap_or_default();
    for message in &request.messages {
        out.push('\n');
        out.push_str(&message.text());
    }
    out
}

fn plan_nudges(request: &taurus_provider::ChatRequest) -> usize {
    request
        .messages
        .iter()
        .filter(|m| {
            m.content.iter().any(|b| {
                matches!(b, ContentBlock::Text { text } if text.contains("still has steps that are not marked done"))
            })
        })
        .count()
}

#[tokio::test]
async fn a_turn_that_stops_with_steps_open_is_asked_to_close_them() {
    // The reported failure, end to end. The model does all the work, marks the
    // last step active, runs it, and reports finishing in prose — leaving a
    // checklist that says 1 of 2 and a panel that would say so tomorrow. Being
    // told in the prompt to close the list does not fix it: the plan is in the
    // system prompt on every one of these iterations and it stops anyway.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([
                {"text": "Change the greeting", "state": "active"},
                {"text": "Add the version field", "state": "todo"},
            ]),
        ),
        plan_call(
            "t2",
            serde_json::json!([
                {"text": "Change the greeting", "state": "done"},
                {"text": "Add the version field", "state": "active"},
            ]),
        ),
        ScriptedTurn::text("All done — both steps are finished."),
        plan_call(
            "t3",
            serde_json::json!([
                {"text": "Change the greeting", "state": "done"},
                {"text": "Add the version field", "state": "done"},
            ]),
        ),
        ScriptedTurn::text("Done."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    // Asked once, and the turn ran on rather than ending on the prose.
    let last = provider.last_request().await.unwrap();
    assert_eq!(plan_nudges(&last), 1);
    // And the list it went back and closed is the one the request now carries.
    let sent = plan_as_sent(&last);
    assert!(sent.contains("[x] Add the version field"), "{sent}");
}

#[tokio::test]
async fn a_plan_with_every_step_closed_is_left_alone() {
    // The nudge costs a round trip, so it has to be silent on the turns that
    // did the bookkeeping themselves — which is most of them.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([{"text": "Change the greeting", "state": "done"}]),
        ),
        ScriptedTurn::text("Done."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    assert_eq!(provider.request_count().await, 2);
    assert_eq!(plan_nudges(&provider.last_request().await.unwrap()), 0);
}

#[tokio::test]
async fn a_turn_that_means_to_stop_early_is_only_asked_once() {
    // The way out the wording offers has to be real. A turn that genuinely
    // stopped — a question to ask, work it cannot do — says so and finishes,
    // and asking again every time it tried would be an unbreakable loop.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([
                {"text": "Change the greeting", "state": "done"},
                {"text": "Add the version field", "state": "active"},
            ]),
        ),
        ScriptedTurn::text("Stopping here."),
        ScriptedTurn::text("I cannot do the second step: there is no config.toml."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    assert_eq!(provider.request_count().await, 3);
    assert_eq!(plan_nudges(&provider.last_request().await.unwrap()), 1);
}

#[tokio::test]
async fn a_turn_with_no_plan_at_all_is_never_asked_about_one() {
    // Every sub-agent, and every turn the model answered without planning. The
    // board is empty rather than unfinished, and the two must not read alike.
    let (agent, provider, _dir) = planning(vec![ScriptedTurn::text("It is 4pm.")]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("what time is it"), tx)
        .await
        .unwrap();

    assert_eq!(provider.request_count().await, 1);
    assert_eq!(plan_nudges(&provider.last_request().await.unwrap()), 0);
}

#[tokio::test]
async fn a_plan_is_restated_to_the_model_on_every_later_iteration() {
    // The whole feature. A 9B model does not lose the steps because it forgot
    // them — they are still in the history — it loses them because by iteration
    // nine they are twenty messages back behind a wall of tool output. So the
    // plan is not left in the history: it is rebuilt onto the end of every
    // request, and this is the test that it actually arrives there.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([
                {"text": "Read the parser", "state": "active"},
                {"text": "Add the token type", "state": "todo"},
            ]),
        ),
        ScriptedTurn::tool_call("t2", "list_dir", serde_json::json!({"path": "."})),
        ScriptedTurn::text("Done."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    let seen = provider.seen.lock().await;
    // Four, not three: this model says "Done." on a plan it never closed, so
    // the third request is followed by one more asking it to — see
    // `a_turn_that_stops_with_steps_open_is_asked_to_close_them`. It carries
    // the same unclosed plan, which is what the loop below wants anyway.
    assert_eq!(seen.len(), 4);

    // Nothing on the first request: the plan did not exist when it was built.
    let first = plan_as_sent(&seen[0]);
    assert!(!first.contains("[>] Read the parser"), "{first}");

    // On every request after the call, in the state the model set, and last of
    // everything — nearer the model's attention than the system prompt was, and
    // behind a prefix that no longer moves when a step does.
    for request in &seen[1..] {
        let sent = plan_as_sent(request);
        assert!(sent.contains("[>] Read the parser"), "{sent}");
        assert!(sent.contains("[ ] Add the token type"), "{sent}");
        let tail = request.messages.last().expect("a message").text();
        assert!(tail.contains("# Your current plan"), "{tail}");
    }
}

#[tokio::test]
async fn the_prompt_carries_the_plan_as_it_stands_rather_than_as_it_started() {
    // Restated, not captured. A model that marked step one done on iteration
    // two must not read it as still in progress on iteration three — that is
    // the exact drift this exists to stop, reintroduced by caching.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([{"text": "Read the parser", "state": "active"}]),
        ),
        plan_call(
            "t2",
            serde_json::json!([{"text": "Read the parser", "state": "done"}]),
        ),
        ScriptedTurn::text("Done."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    let last = plan_as_sent(&provider.last_request().await.unwrap());
    assert!(last.contains("[x] Read the parser"), "{last}");
    assert!(!last.contains("[>] Read the parser"), "{last}");
}

#[tokio::test]
async fn only_one_copy_of_the_plan_is_ever_in_front_of_the_model() {
    // Stored in the session instead, this would accumulate: one copy per
    // iteration, each staler than the last, leaving the model to work out which
    // of nine checklists is current. It is written onto the copy being sent and
    // never into the session, so there is only ever the live one.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([{"text": "Read the parser", "state": "active"}]),
        ),
        ScriptedTurn::tool_call("t2", "list_dir", serde_json::json!({"path": "."})),
        ScriptedTurn::tool_call("t3", "list_dir", serde_json::json!({"path": "."})),
        ScriptedTurn::text("Done."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    let last = provider.last_request().await.unwrap();
    let sent = plan_as_sent(&last);
    assert_eq!(
        sent.matches("# Your current plan").count(),
        1,
        "the plan was stacked rather than rebuilt: {sent}"
    );
    // And nothing of it is in the session, which is the copy that persists and
    // the one compaction would erode. The request carries it; the record does
    // not.
    let stored = session
        .messages
        .iter()
        .any(|m| m.text().contains("# Your current plan"));
    assert!(!stored, "the plan leaked into the stored conversation");
}

#[tokio::test]
async fn moving_a_step_leaves_the_prompt_prefix_untouched() {
    // Why the plan sits where it does. A backend reuses the longest identical
    // prefix of a prompt it has already processed, and this one is asked to
    // move a step at the start and end of every step it works on. Measured on a
    // local 30B: one 9,550-token prompt costs 16ms to repeat unchanged and
    // 10,933ms to repeat with one line of the plan edited — so anything ahead
    // of the plan is paid for again every time a checkbox moves. On Anthropic
    // it is the same fact with a price on it: the cache breakpoint sits on the
    // system field and covers the tools rendered before it.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([
                {"text": "Read the parser", "state": "active"},
                {"text": "Add the token type", "state": "todo"},
            ]),
        ),
        plan_call(
            "t2",
            serde_json::json!([
                {"text": "Read the parser", "state": "done"},
                {"text": "Add the token type", "state": "active"},
            ]),
        ),
        ScriptedTurn::text("Done."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    let seen = provider.seen.lock().await;
    let plans: Vec<String> = seen.iter().map(plan_as_sent).collect();
    assert!(plans.last().unwrap().contains("[x] Read the parser"));

    // The plan moved. Nothing ahead of it did.
    for pair in seen.windows(2) {
        assert_eq!(
            pair[0].system, pair[1].system,
            "the system prompt moved between iterations"
        );
        assert_eq!(
            pair[0].tools, pair[1].tools,
            "the tool schemas moved between iterations"
        );
    }
}

#[tokio::test]
async fn a_turn_that_never_plans_carries_no_planning_text_at_all() {
    // The cost of this feature for every short turn has to be zero — not a
    // small section, not an empty heading. A standing instruction to keep a
    // checklist is exactly how a two-step turn grows a six-step plan.
    let (agent, provider, _dir) = planning(vec![ScriptedTurn::text("Hello there")]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("hi"), tx)
        .await
        .unwrap();

    let system = provider
        .last_request()
        .await
        .unwrap()
        .system
        .unwrap_or_default();
    assert!(!system.contains("plan"), "{system}");
}

#[tokio::test]
async fn a_refused_plan_leaves_the_previous_one_standing() {
    // A model that sends two steps in progress gets an error, and the plan it
    // set a moment ago has to survive that — a rejected update that wiped the
    // checklist would be worse than one that was accepted wrongly.
    let (agent, provider, _dir) = planning(vec![
        plan_call(
            "t1",
            serde_json::json!([{"text": "Read the parser", "state": "active"}]),
        ),
        plan_call(
            "t2",
            serde_json::json!([
                {"text": "Read the parser", "state": "active"},
                {"text": "Add the token type", "state": "active"},
            ]),
        ),
        ScriptedTurn::text("Done."),
    ]);

    let mut session = Session::new("fake");
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent
        .run_turn(&mut session, Message::user("do the thing"), tx)
        .await
        .unwrap();

    let last = provider.last_request().await.unwrap();
    let sent = plan_as_sent(&last);
    assert!(sent.contains("[>] Read the parser"), "{sent}");
    assert!(!sent.contains("[>] Add the token type"), "{sent}");
    // And the model was told what was wrong with it, in terms it can act on.
    let told = last.messages.iter().any(|m| {
        m.text().contains("exactly one step may be in progress")
            || m.content.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::ToolResult { content, .. }
                        if content.to_text().contains("exactly one step may be in progress")
                )
            })
    });
    assert!(told, "the model was not told why: {:#?}", last.messages);
}

#[tokio::test]
async fn a_recorded_turn_is_written_down_as_it_runs() {
    let spy = Arc::new(Spy::default());
    let h = recording(
        harness(vec![
            ScriptedTurn::tool_call("t1", "list_dir", serde_json::json!({"path": "."})),
            ScriptedTurn::text("Nothing much in there."),
        ]),
        spy.clone(),
    );
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "look around").await;
    assert_eq!(outcome.unwrap().iterations, 2);

    let snapshots = spy.snapshots.lock().await.clone();
    // Three times: the question before the model is asked it, the first round's
    // results as they land, and the finished turn. Each one is a point a crash
    // could happen at and still leave something worth having — and the first is
    // what makes a conversation listable, with its title, while it is being
    // answered rather than only once it has been.
    assert_eq!(snapshots.len(), 3, "{snapshots:?}");
    assert_eq!(
        snapshots[0], 1,
        "the question is written down before the request that answers it"
    );
    assert!(snapshots[0] < snapshots[1], "{snapshots:?}");
    assert!(snapshots[1] < snapshots[2], "{snapshots:?}");
    assert_eq!(snapshots[2], session.messages.len());
}

#[tokio::test]
async fn a_turn_that_failed_is_recorded_too() {
    let spy = Arc::new(Spy::default());
    let h = recording(
        harness(vec![ScriptedTurn::permanent_failure()]),
        spy.clone(),
    );
    let mut session = Session::new("fake");
    let (outcome, _) = run(&h, &mut session, "ask something").await;
    assert!(outcome.is_err(), "the script fails in a way no retry fixes");

    // The transcript of a turn that broke is worth more than the transcript of
    // one that went fine, not less. Twice, both holding only the question: once
    // before the request that failed, once after it did. A turn that produced
    // nothing still leaves what was asked.
    let snapshots = spy.snapshots.lock().await.clone();
    assert_eq!(snapshots, vec![1, session.messages.len()]);
}
