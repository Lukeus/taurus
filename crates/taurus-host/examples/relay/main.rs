//! One long task, run under each context strategy, on a live model.
//!
//! ```sh
//! cargo run -p taurus-host --example relay -- <model> [runs] [window]
//! ```
//!
//! `runs` is how many times each strategy runs (2 by default), and `window`
//! is the context Ollama is asked to allocate (12,288 by default). The window
//! is set small on purpose: the question is what happens to a turn that
//! outgrows its window several times over, and on the default 32,768 this
//! task would fit with room to spare.
//!
//! The task is eight functions in a Python module, a spec to follow and 25
//! tests to pass, worked one function at a time. That's long enough to fill a
//! 12k window two or three times on a 9B model, and it ends in a number that
//! doesn't depend on reading the transcript: how many tests pass.
//!
//! Each strategy runs through the same `Agent` the app builds — the real
//! system prompt, the plan tool, checkpoints — with `compact` as the control
//! and `relay` as the strategy under test. Printed per run: tests passing,
//! how the turn ended, rounds, how many times the history was summarized or
//! handed over, tokens billed, and wall time. Nothing is asserted beyond the
//! run finishing: this is a measurement, not a gate, and a 9B model
//! legitimately fails this task some of the time under either strategy.
//!
//! Tokens billed don't include the summary or notes requests themselves,
//! which go outside the turn's accounting; the compaction and handover
//! counts say how many of those there were, and wall time includes them.
//!
//! Needs Ollama and `python3`. It writes only inside temporary directories.

use std::sync::Arc;
use std::time::Instant;

use taurus_core::{Agent, AgentConfig, ContextStrategy, Session, UiEvent};
use taurus_provider::{Message, Provider};
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_tools::builtin::plan::UpdatePlan;
use taurus_tools::{
    AllowAll, CheckpointStore, PermissionEngine, PlanBoard, ToolContext, ToolRegistry,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SPEC: &str = include_str!("SPEC.md");
const MODULE: &str = include_str!("inventory.py");
const TESTS: &str = include_str!("test_inventory.py");

const TASK: &str = "\
Implement every function in inventory.py so that all the tests in \
test_inventory.py pass. SPEC.md says what each function does. Work through \
the functions one at a time, and run `python3 -m unittest -q` after each one. \
Don't change the tests.";

/// More than the task needs on a good run, so a run that ends on the ceiling
/// has really stopped making progress rather than being cut short.
const MAX_ITERATIONS: u32 = 60;

/// How many tests `test_inventory.py` holds.
const TEST_COUNT: u32 = 25;

#[derive(Default)]
struct Run {
    passed: u32,
    total: u32,
    ended: String,
    rounds: u32,
    trimmed: u32,
    compacted: u32,
    legs: u32,
    legs_without_notes: u32,
    input_tokens: u64,
    output_tokens: u64,
    seconds: f64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().unwrap_or_else(|| "ornith-1.5:9b".into());
    let runs: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(2);
    let window: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(12_288);

    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL).with_context_limit(Some(window)));
    let caps = provider.capabilities(&model).await?;
    println!(
        "model {model}, window {} (asked for {window}), native tools {}, {runs} run(s) each\n",
        caps.context_length, caps.native_tools
    );

    let mut results: Vec<(ContextStrategy, Run)> = Vec::new();
    for run in 0..runs {
        // Interleaved rather than all of one then all of the other, so a
        // machine that warms up or slows down over the session doesn't
        // favor whichever strategy happened to go second.
        for strategy in [ContextStrategy::Compact, ContextStrategy::Relay] {
            let label = name(strategy);
            println!("── {label}, run {} ──", run + 1);
            let result = once(provider.clone(), &model, strategy).await?;
            print_row(label, &result);
            results.push((strategy, result));
        }
    }

    println!("\nsummary (mean over {runs} run(s))");
    println!(
        "{:<8} {:>7} {:>7} {:>6} {:>6} {:>10} {:>8}",
        "strategy", "tests", "rounds", "sums", "legs", "in tokens", "seconds"
    );
    for strategy in [ContextStrategy::Compact, ContextStrategy::Relay] {
        let mine: Vec<&Run> = results
            .iter()
            .filter(|(s, _)| *s == strategy)
            .map(|(_, r)| r)
            .collect();
        let n = mine.len() as f64;
        let mean = |f: &dyn Fn(&Run) -> f64| mine.iter().map(|r| f(r)).sum::<f64>() / n;
        println!(
            "{:<8} {:>4.1}/{:<2} {:>7.1} {:>6.1} {:>6.1} {:>10.0} {:>8.0}",
            name(strategy),
            mean(&|r| r.passed as f64),
            mine.first().map_or(0, |r| r.total),
            mean(&|r| r.rounds as f64),
            mean(&|r| r.compacted as f64),
            mean(&|r| r.legs as f64),
            mean(&|r| r.input_tokens as f64),
            mean(&|r| r.seconds),
        );
    }
    Ok(())
}

async fn once(
    provider: Arc<OllamaProvider>,
    model: &str,
    strategy: ContextStrategy,
) -> Result<Run, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let workspace = dir.path().canonicalize()?;
    std::fs::write(workspace.join("SPEC.md"), SPEC)?;
    std::fs::write(workspace.join("inventory.py"), MODULE)?;
    std::fs::write(workspace.join("test_inventory.py"), TESTS)?;
    let logs = tempfile::tempdir()?;

    let permissions = Arc::new(PermissionEngine::new(
        &workspace,
        workspace.join(".taurus"),
        Box::new(AllowAll),
    ));
    let cancel = CancellationToken::new();
    let recorder = CheckpointStore::new(logs.path()).begin_turn("relay", &workspace, TASK);
    let plan = PlanBoard::new();
    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(UpdatePlan::new(plan.clone())));
    let agent = Agent::new(
        provider,
        registry,
        ToolContext::new(workspace.clone(), permissions, cancel).with_checkpoints(recorder),
        AgentConfig {
            system_prompt: taurus_host::prompt::build(&workspace, None, None, None, false, false),
            max_iterations: MAX_ITERATIONS,
            context_strategy: strategy,
            ..Default::default()
        },
    )
    .with_plan(plan);

    let (tx, mut rx) = mpsc::channel(1024);
    let counter = tokio::spawn(async move {
        let mut run = Run::default();
        while let Some(event) = rx.recv().await {
            match event {
                UiEvent::IterationStarted { iteration } => run.rounds = iteration,
                UiEvent::ToolCallStarted { name, .. } => print!("{name} "),
                UiEvent::ContextTrimmed { .. } => run.trimmed += 1,
                UiEvent::Compacted { messages_removed } => {
                    run.compacted += 1;
                    print!("\n  [summarized {messages_removed} messages]\n");
                }
                UiEvent::Relayed {
                    leg,
                    messages_removed,
                    notes,
                } => {
                    run.legs += 1;
                    if !notes {
                        run.legs_without_notes += 1;
                    }
                    print!(
                        "\n  [leg {leg}: {messages_removed} messages handed over, notes {notes}]\n"
                    );
                }
                UiEvent::Error { message } => print!("\n  [error] {message}\n"),
                _ => {}
            }
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
        run
    });

    let started = Instant::now();
    let mut session = Session::new(model);
    let outcome = agent.run_turn(&mut session, Message::user(TASK), tx).await;
    let seconds = started.elapsed().as_secs_f64();
    drop(agent);
    let mut run = counter.await?;
    println!();

    run.seconds = seconds;
    run.ended = match &outcome {
        Ok(done) => format!("{:?}", done.stop_reason),
        Err(error) => error.kind().to_string(),
    };
    run.input_tokens = session.usage.input_tokens as u64;
    run.output_tokens = session.usage.output_tokens as u64;
    (run.passed, run.total) = score(&workspace).await;
    Ok(run)
}

/// Tests passing out of tests run, from unittest's own summary. The test
/// file is written back first, so a model that edited it is scored against
/// the real tests.
async fn score(workspace: &std::path::Path) -> (u32, u32) {
    let _ = std::fs::write(workspace.join("test_inventory.py"), TESTS);
    let output = tokio::process::Command::new("python3")
        .args(["-m", "unittest", "-q"])
        .current_dir(workspace)
        .output()
        .await;
    let Ok(output) = output else {
        return (0, 0);
    };
    let text = String::from_utf8_lossy(&output.stderr);
    let ran: u32 = text
        .lines()
        .find_map(|l| l.strip_prefix("Ran ")?.split(' ').next()?.parse().ok())
        .unwrap_or(0);
    // An import error runs one synthetic failing test, not the real ones,
    // and that's no tests passing rather than all but one.
    if ran != TEST_COUNT {
        return (0, TEST_COUNT);
    }
    // "FAILED (failures=2, errors=1)", or "OK".
    let broken: u32 = text
        .lines()
        .find(|l| l.starts_with("FAILED"))
        .map(|l| {
            l.split(|c: char| !c.is_ascii_digit())
                .filter_map(|n| n.parse::<u32>().ok())
                .sum()
        })
        .unwrap_or(0);
    (TEST_COUNT.saturating_sub(broken), TEST_COUNT)
}

fn name(strategy: ContextStrategy) -> &'static str {
    match strategy {
        ContextStrategy::Compact => "compact",
        ContextStrategy::Relay => "relay",
    }
}

fn print_row(label: &str, r: &Run) {
    println!(
        "{label}: {}/{} tests, ended {}, {} rounds, {} trims, {} summaries, {} legs ({} without notes), \
         {} in / {} out tokens, {:.0}s\n",
        r.passed,
        r.total,
        r.ended,
        r.rounds,
        r.trimmed,
        r.compacted,
        r.legs,
        r.legs_without_notes,
        r.input_tokens,
        r.output_tokens,
        r.seconds
    );
}
