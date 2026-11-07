//! Two delegates that write, at once, each in its own git worktree, on a live
//! model.
//!
//! `cargo run -p taurus-host --example parallel_writers -- <model> [runs]`
//!
//! Two parts, each run `runs` times (2 by default):
//!
//! 1. **Direct.** Two `coder` delegations with `isolate`, started together
//!    through the same `SpawnSubagent` and `Worktrees` the app uses, each
//!    asked to change a different file. Asserted: both report; the workspace
//!    is exactly as it was (nothing reached it); every run that changed
//!    something is kept as a commit git can show; no worktree folder is left
//!    behind. Measured, not asserted: how much the two overlapped in time,
//!    and whether each changed the file it was asked to.
//! 2. **Turn.** A parent turn asked to run both at once and then apply them
//!    with `apply_delegate`. Asserted: the workspace changes only through an
//!    apply that worked. Printed: whether the model asked for both in one
//!    round, whether it applied them, and what the files ended up holding.
//!
//! Needs Ollama and git. It writes only inside temporary directories.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use taurus_core::{Agent, AgentConfig, ApplyDelegate, Session, SpawnSubagent};
use taurus_host::worktree::Worktrees;
use taurus_provider::Message;
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_tools::{
    AllowAll, CheckpointStore, DelegateReport, PermissionEngine, Tool, ToolContext, ToolProgress,
    ToolRegistry,
};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

/// Keeps the report a delegation sends its card.
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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with two files to change, committed.
fn repository() -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "live@example.com"]);
    git(root, &["config", "user.name", "Live"]);
    std::fs::write(root.join("a.txt"), "one\n")?;
    std::fs::write(root.join("b.txt"), "two\n")?;
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "init"]);
    Ok(dir)
}

fn context(root: &Path, logs: &Path) -> ToolContext {
    ToolContext::new(
        root.to_path_buf(),
        Arc::new(PermissionEngine::new(
            root,
            root.join(".taurus"),
            Box::new(AllowAll),
        )),
        CancellationToken::new(),
    )
    .with_checkpoints(CheckpointStore::new(logs).begin_turn("live", root, "live"))
    .with_session("live")
}

fn read(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join(name)).unwrap_or_default()
}

const TURN_PROMPT: &str = "Use two `coder` sub-agents at the same time, both with `isolate` set \
     to true, in a single round: one rewrites a.txt to contain exactly the line alpha, the other \
     rewrites b.txt to contain exactly the line beta. When both have reported, apply each one's \
     run with `apply_delegate`, then say what a.txt and b.txt contain.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.5:9b-mlx".into());
    let runs: usize = std::env::args()
        .nth(2)
        .and_then(|n| n.parse().ok())
        .unwrap_or(2);
    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL.to_string()));

    println!("Direct: two isolated coders at once on {model}, {runs} runs…\n");
    for run in 1..=runs {
        let repo = repository()?;
        let root = repo.path().canonicalize()?;
        let home = tempfile::tempdir()?;
        let trees = Arc::new(Worktrees::new(home.path().join("worktrees")));
        let logs = tempfile::tempdir()?;
        let spawn = Arc::new(
            SpawnSubagent::new(
                provider.clone(),
                Arc::new(RwLock::new(ToolRegistry::with_builtins())),
                model.clone(),
                2,
            )
            .with_defaults(AgentConfig {
                verify_changes: false,
                ..Default::default()
            })
            .with_isolation(trees.clone()),
        );

        let one = |file: &'static str, word: &'static str| {
            let spawn = spawn.clone();
            let card = Arc::new(Card::default());
            let mut ctx = context(&root, logs.path());
            ctx.progress = Some(card.clone());
            async move {
                let started = Instant::now();
                let text = spawn
                    .execute(
                        serde_json::json!({
                            "agent_type": "coder",
                            "isolate": true,
                            "prompt": format!(
                                "Rewrite {file} so it contains exactly the line {word}. Change no \
                                 other file. Nothing needs to be run afterwards."
                            ),
                        }),
                        &ctx,
                    )
                    .await;
                (
                    file,
                    text,
                    card.report.lock().await.clone(),
                    started.elapsed(),
                )
            }
        };
        let wall = Instant::now();
        let (a, b) = tokio::join!(one("a.txt", "alpha"), one("b.txt", "beta"));
        let wall = wall.elapsed();

        assert_eq!(
            read(&root, "a.txt"),
            "one\n",
            "a delegate wrote into the workspace"
        );
        assert_eq!(
            read(&root, "b.txt"),
            "two\n",
            "a delegate wrote into the workspace"
        );
        assert_eq!(
            git(&root, &["worktree", "list"]).lines().count(),
            1,
            "a copy was left"
        );
        let mut each = 0f32;
        for (file, text, report, took) in [a, b] {
            let text = text?.to_text().into_owned();
            let report = report.expect("every delegation sends its card a report");
            assert!(text.starts_with("Status: "), "no status line: {text}");
            each += took.as_secs_f32();
            let kept = report.kept.as_ref();
            if let Some(kept) = kept {
                let shown = git(
                    &root,
                    &[
                        "show",
                        &format!("refs/taurus/delegates/{}/result:{file}", kept.run),
                    ],
                );
                println!(
                    "  run {run} {file}: {} in {:.1}s, kept as {} ({:?}), {file} there says {shown:?}",
                    report.disposition.as_str(),
                    took.as_secs_f32(),
                    kept.run,
                    kept.files
                );
            } else {
                println!(
                    "  run {run} {file}: {} in {:.1}s, changed nothing",
                    report.disposition.as_str(),
                    took.as_secs_f32()
                );
            }
        }
        println!(
            "  run {run}: {:.1}s on the clock for {each:.1}s of work ({:.0}% overlap)\n",
            wall.as_secs_f32(),
            100.0 * (each - wall.as_secs_f32()).max(0.0) / wall.as_secs_f32().max(0.001)
        );
    }

    println!("Turn: the parent runs both and applies them…\n");
    for run in 1..=runs {
        let repo = repository()?;
        let root = repo.path().canonicalize()?;
        let home = tempfile::tempdir()?;
        let trees = Arc::new(Worktrees::new(home.path().join("worktrees")));
        let logs = tempfile::tempdir()?;
        let mut registry = ToolRegistry::with_builtins();
        registry.register(Arc::new(
            SpawnSubagent::new(
                provider.clone(),
                Arc::new(RwLock::new(ToolRegistry::with_builtins())),
                model.clone(),
                2,
            )
            .with_defaults(AgentConfig {
                verify_changes: false,
                ..Default::default()
            })
            .with_isolation(trees.clone()),
        ));
        registry.register(Arc::new(ApplyDelegate::new(trees.clone())));
        let agent = Agent::new(
            provider.clone(),
            registry,
            context(&root, logs.path()),
            AgentConfig {
                verify_changes: false,
                ..Default::default()
            },
        );
        let mut session = Session::new(&model);
        session.id = "live".into();
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let started = Instant::now();
        let outcome = agent
            .run_turn(&mut session, Message::user(TURN_PROMPT), tx)
            .await;
        drain.await?;

        let rounds_with_two = session
            .messages
            .iter()
            .filter(|m| {
                m.tool_uses()
                    .filter(|(_, name, input)| {
                        *name == "spawn_subagent" && input["isolate"] == true
                    })
                    .count()
                    >= 2
            })
            .count();
        let applied: Vec<String> = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|block| match block {
                taurus_provider::ContentBlock::ToolResult {
                    content, is_error, ..
                } if !is_error && content.to_text().starts_with("Applied run ") => {
                    Some(content.to_text().into_owned())
                }
                _ => None,
            })
            .collect();
        let (a, b) = (read(&root, "a.txt"), read(&root, "b.txt"));
        if applied.is_empty() {
            assert_eq!(a, "one\n", "a.txt changed without an apply");
            assert_eq!(b, "two\n", "b.txt changed without an apply");
        }
        println!(
            "  run {run}: {:.1}s, {}, both asked in one round: {}, applied {}: a.txt={a:?} b.txt={b:?}",
            started.elapsed().as_secs_f32(),
            match &outcome {
                Ok(_) => "finished".to_string(),
                Err(e) => format!("stopped ({e})"),
            },
            rounds_with_two > 0,
            applied.len()
        );
    }
    Ok(())
}
