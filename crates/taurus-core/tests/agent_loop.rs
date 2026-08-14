//! End-to-end tests for the agent loop against a scripted provider.

use std::sync::Arc;

use taurus_core::testing::{FakeProvider, ScriptedTurn};
use taurus_core::{Agent, AgentConfig, AgentError, Session, UiEvent};
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
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let cancel = CancellationToken::new();
    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        prompt,
    ));
    let tools = ToolContext::new(workspace.clone(), permissions, cancel.clone());
    let provider = FakeProvider::with_context_length(turns, context_length);
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
                content.contains("denied") && content.contains("Do not retry")
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
                content.contains("<tool_call>")
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
            vec![ContentBlock::ToolUse {
                id: format!("t{i}"),
                name: "read_file".into(),
                input: serde_json::json!({ "path": "a.rs" }),
            }],
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
    assert!(trimmed.contains("called again later"), "{trimmed}");
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
