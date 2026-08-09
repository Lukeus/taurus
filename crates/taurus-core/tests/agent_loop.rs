//! End-to-end tests for the agent loop against a scripted provider.

use std::sync::Arc;

use taurus_core::testing::{FakeProvider, ScriptedTurn};
use taurus_core::{Agent, AgentConfig, AgentError, Session, UiEvent};
use taurus_provider::{Message, StopReason};
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
    let permissions = Arc::new(PermissionEngine::new(&workspace, prompt));
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
    let permissions = Arc::new(PermissionEngine::new(&workspace, Box::new(AllowAll)));
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
