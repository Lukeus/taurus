//! How often a live model hands its result back with `finish`.
//!
//! `cargo run -p taurus-host --example delegate -- <model> [runs]`
//!
//! A delegate that never calls `finish` still reports: the harness builds an
//! `unreported` report from its last message. So the question this answers
//! isn't whether delegation works on a small model. It's how often the
//! fallback is the normal path, which decides how much the typed report is
//! worth on the machine it's run on.
//!
//! Three tasks, each run `runs` times (3 by default) through the same
//! `SpawnSubagent` the app uses:
//!
//! 1. **found**: a question the workspace answers. Expect `done`.
//! 2. **undecidable**: a question the workspace can't answer. Expect
//!    `blocked` on the user, or `failed`.
//! 3. **write**: a file to create. Expect `done`, with the file listed.
//!
//! Asserted: every report has a status line, and a `done` write lists the
//! file it wrote. Printed, not asserted: how often `finish` was called and
//! what it said, because a model may legitimately get those wrong and this is
//! a measurement, not a gate.
//!
//! Needs Ollama. It writes only inside temporary directories.

use std::sync::Arc;

use async_trait::async_trait;
use taurus_core::{Agent, AgentConfig, Session, SpawnSubagent};
use taurus_provider::Message;
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_tools::{
    AllowAll, CheckpointStore, DelegateReport, Disposition, PermissionEngine, Tool, ToolContext,
    ToolProgress, ToolRegistry,
};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

struct Task {
    name: &'static str,
    agent: &'static str,
    prompt: &'static str,
}

const TASKS: [Task; 3] = [
    Task {
        name: "found",
        agent: "explorer",
        prompt: "Which file in this workspace defines the function `parse_port`? Report the \
                 file's path.",
    },
    Task {
        name: "undecidable",
        agent: "explorer",
        prompt: "The server's port is set in both config.toml and config.local.toml, and they \
                 disagree. Work out which one the server actually reads at startup, and report \
                 the port it listens on.",
    },
    Task {
        name: "write",
        agent: "worker",
        prompt: "Create a file named notes.txt in the workspace root containing exactly the \
                 line: hello. Nothing needs to be run afterwards.",
    },
];

/// Keeps the report the delegation sends to the card.
#[derive(Default)]
struct Card {
    report: Mutex<Option<DelegateReport>>,
}

#[async_trait]
impl ToolProgress for Card {
    async fn step(&self, _label: String) {}

    async fn delegate_report(&self, report: DelegateReport) {
        *self.report.lock().await = Some(report);
    }
}

/// A fresh workspace per run, so one run's `notes.txt` can't answer the next.
fn workspace() -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("src"))?;
    std::fs::write(
        dir.path().join("src/net.rs"),
        "pub fn parse_port(s: &str) -> Option<u16> {\n    s.trim().parse().ok()\n}\n",
    )?;
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n")?;
    // Nothing in the workspace says which of these is read.
    std::fs::write(dir.path().join("config.toml"), "port = 8080\n")?;
    std::fs::write(dir.path().join("config.local.toml"), "port = 9090\n")?;
    Ok(dir)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.5:9b-mlx".into());
    let runs: usize = std::env::args()
        .nth(2)
        .and_then(|n| n.parse().ok())
        .unwrap_or(3);
    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL.to_string()));

    println!(
        "Delegating {} tasks × {runs} runs to {model}…\n",
        TASKS.len()
    );

    let mut called = 0;
    let mut total = 0;
    for task in &TASKS {
        for run in 1..=runs {
            let dir = workspace()?;
            let logs = tempfile::tempdir()?;
            let recorder = CheckpointStore::new(logs.path()).begin_turn("live", dir.path(), "live");
            let card = Arc::new(Card::default());
            let ctx = ToolContext::new(
                dir.path().to_path_buf(),
                Arc::new(PermissionEngine::new(
                    dir.path(),
                    dir.path().join(".taurus"),
                    Box::new(AllowAll),
                )),
                CancellationToken::new(),
            )
            .with_checkpoints(recorder)
            .with_progress(card.clone());

            let spawn = SpawnSubagent::new(
                provider.clone(),
                Arc::new(RwLock::new(ToolRegistry::with_builtins())),
                model.clone(),
                1,
            );
            let started = std::time::Instant::now();
            let text = spawn
                .execute(
                    serde_json::json!({ "agent_type": task.agent, "prompt": task.prompt }),
                    &ctx,
                )
                .await?
                .to_text()
                .into_owned();
            let report = card
                .report
                .lock()
                .await
                .clone()
                .expect("every delegation sends its card a report");

            assert!(text.starts_with("Status: "), "no status line: {text}");
            if task.name == "write" && report.disposition == Disposition::Done {
                assert!(
                    report.files.iter().any(|f| f == "notes.txt"),
                    "a done write must list what it wrote: {:?}",
                    report.files
                );
            }

            total += 1;
            let reported = !matches!(
                report.disposition,
                Disposition::Unreported | Disposition::Cancelled
            ) && !report.summary.starts_with("It stopped early");
            if reported {
                called += 1;
            }
            let who = match (report.owner, &report.needs) {
                (Some(owner), Some(needs)) => format!(" [{owner:?}: {needs}]"),
                _ => String::new(),
            };
            println!(
                "{:<12} run {run}  {:<10}{who}  files={:?}  {:.1}s",
                task.name,
                report.disposition.as_str(),
                report.files,
                started.elapsed().as_secs_f32()
            );
            if !reported {
                let why = report.summary.lines().next().unwrap_or("");
                println!("{:<12}        fell back: {why}", "");
            }
        }
    }

    println!(
        "\n`finish` was called in {called} of {total} delegations ({:.0}%). The rest fell back \
         to an `unreported` or `failed` report built by the harness.",
        100.0 * called as f32 / total.max(1) as f32
    );

    // A parent turn, this time, because background delegation only exists
    // inside one: the report has to reach a turn that is still going.
    println!("\nBackground: a parent turn that starts an explorer and keeps working…\n");
    let mut delivered = 0;
    for run in 1..=runs {
        let dir = workspace()?;
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(PermissionEngine::new(
                dir.path(),
                dir.path().join(".taurus"),
                Box::new(AllowAll),
            )),
            CancellationToken::new(),
        );
        let mut registry = ToolRegistry::with_builtins();
        registry.register(Arc::new(SpawnSubagent::new(
            provider.clone(),
            Arc::new(RwLock::new(ToolRegistry::with_builtins())),
            model.clone(),
            2,
        )));
        let agent = Agent::new(provider.clone(), registry, ctx, AgentConfig::default());
        let mut session = Session::new(&model);
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let started = std::time::Instant::now();
        let outcome = agent
            .run_turn(&mut session, Message::user(BACKGROUND_PROMPT), tx)
            .await;
        drain.await?;

        let used_background = session.messages.iter().any(|m| {
            m.tool_uses()
                .any(|(_, name, input)| name == "spawn_subagent" && input["background"] == true)
        });
        let reports = session
            .messages
            .iter()
            .filter(|m| m.text().contains("<background-report"))
            .count();
        let answer = session
            .messages
            .last()
            .map(|m| m.text())
            .unwrap_or_default();
        // The turn must never end with a report it hasn't read.
        if used_background {
            assert!(
                outcome.is_err() || reports == 1,
                "a finished turn that started background work read {reports} reports"
            );
        }
        if used_background && reports == 1 {
            delivered += 1;
        }
        println!(
            "run {run}  background={used_background}  reports={reports}  answer names net.rs={}  \
             {:.1}s{}",
            answer.contains("net.rs"),
            started.elapsed().as_secs_f32(),
            outcome
                .err()
                .map(|e| format!("  ({e})"))
                .unwrap_or_default()
        );
    }
    println!("\nA background report was started and read in {delivered} of {runs} turns.");
    Ok(())
}

const BACKGROUND_PROMPT: &str = "\
Use spawn_subagent with background set to true to have an explorer find which \
file defines the function `parse_port`. While it works, read src/main.rs \
yourself. Then answer with both: the file that defines parse_port, and what \
src/main.rs contains.";
