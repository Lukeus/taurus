//! A turn killed in the middle of a tool call, reopened, and continued.
//!
//! `cargo build -p taurus-cli && cargo run -p taurus-cli --example interrupt -- <model>`
//!
//! Runs the real `taurus` binary in a scratch folder and asks it to run
//! `sleep 30`. Once the transcript shows that call written down, the process
//! is killed, which is as close to pulling the plug as a test gets. Then it
//! checks the three things durable turns promise:
//!
//! 1. **The call was on disk before it ran**, so the reopened conversation
//!    knows what was running.
//! 2. **It reads as interrupted, and the call is owed "outcome unknown"**,
//!    naming the command to check, and not replayed.
//! 3. **`/continue` picks it up** in a fresh process, and the transcript it
//!    leaves answers every call it holds.
//!
//! Needs Ollama, or whatever provider `-m` resolves to. It leaves one
//! conversation behind, filed under the scratch folder (passed with `-w`)
//! rather than any real workspace.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use taurus_host::sessions;
use taurus_provider::{ContentBlock, Role};

/// Explicit about the foreground: left to itself, a model may start a long
/// `sleep` in the background, where the call returns at once and there's no
/// moment with it running to kill.
const TASK: &str = "Use run_command to run the shell command `sleep 30` in the foreground (don't \
                    set background), wait for it to finish, then tell me it finished.";

fn taurus() -> PathBuf {
    // examples/interrupt → the binary two levels up, in the same profile.
    let exe = std::env::current_exe().expect("own path");
    let bin = exe
        .parent()
        .and_then(Path::parent)
        .map(|dir| dir.join(format!("taurus{}", std::env::consts::EXE_SUFFIX)))
        .expect("a target directory");
    assert!(
        bin.is_file(),
        "{} isn't built. Run `cargo build -p taurus-cli` first.",
        bin.display()
    );
    bin
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.5:9b-mlx".into());
    let workspace = tempfile::tempdir()?;
    let workspace_path = workspace.path().canonicalize()?;
    let bin = taurus();
    // Named, not inherited from the working directory: without `-w` the CLI
    // opens the workspace it was last used in, and this would file its
    // conversation, and run its command, in somebody's real project.
    let workspace_arg = workspace_path.to_str().expect("a UTF-8 temp path");

    println!("Asking {model} to run `sleep 30`, and killing it mid-call…");
    let mut child = Command::new(&bin)
        .current_dir(&workspace_path)
        .args([
            "run",
            "-w",
            workspace_arg,
            "-m",
            &model,
            "--allow-command",
            "sleep",
            TASK,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;

    // 1. Wait for the call to be written down, then kill the process.
    let deadline = Instant::now() + Duration::from_secs(180);
    let id = loop {
        assert!(
            Instant::now() < deadline,
            "the call never reached the transcript (did the model run it?)"
        );
        if let Some(meta) = sessions::latest(&workspace_path) {
            if let Ok(loaded) = sessions::load(&meta.id) {
                let running = loaded.session.messages.last().is_some_and(|m| {
                    m.role == Role::Assistant
                        && m.tool_uses().any(|(_, name, _)| name == "run_command")
                });
                if running {
                    break meta.id;
                }
            }
        }
        if let Some(status) = child.try_wait()? {
            panic!("taurus exited ({status}) before the call was written down");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    child.kill()?;
    child.wait()?;
    println!("  killed it with `sleep` running, in conversation {id}");

    // 2. Reopened, it reads as interrupted, with the call owed an answer.
    let loaded = sessions::load(&id)?.session;
    let interrupted = loaded
        .interrupted
        .clone()
        .expect("a turn with a start and no end is an interrupted one");
    assert_eq!(interrupted.unanswered, 1, "{interrupted:?}");
    assert_eq!(interrupted.attempts, 1);
    let owed = match loaded.owed.first() {
        Some(ContentBlock::ToolResult { content, .. }) => content.to_text().into_owned(),
        other => panic!("the running call must be owed a result: {other:?}"),
    };
    assert!(owed.starts_with("Outcome unknown"), "{owed}");
    assert!(owed.contains("sleep"), "it names what to check: {owed}");
    println!("  reopened: interrupted, 1 call owed \"{owed}\"");

    // 3. Continued in a fresh process.
    println!("Continuing with /continue…");
    let status = Command::new(&bin)
        .current_dir(&workspace_path)
        .args([
            "run",
            "-w",
            workspace_arg,
            "-m",
            &model,
            "--allow-command",
            "sleep",
            "--resume",
            &id,
            "/continue",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()?;
    assert!(status.success(), "the continued turn failed: {status}");

    let after = sessions::load(&id)?.session;
    assert!(after.interrupted.is_none(), "the continuation finished");
    // Every call in the transcript is answered by the message after it: what
    // every provider requires, and what the owed result is for.
    for (i, message) in after.messages.iter().enumerate() {
        for (call, _, _) in message.tool_uses() {
            let answered = after.messages.get(i + 1).is_some_and(|next| {
                next.content.iter().any(|b| {
                    matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == call)
                })
            });
            assert!(
                answered,
                "call {call} in message {i} has no result after it"
            );
        }
    }
    let answer = after.messages.last().map(|m| m.text()).unwrap_or_default();
    println!("  continued and finished: \"{}\"", answer.trim());
    println!("\nEvery call in the transcript is answered, and nothing was replayed blind.");
    Ok(())
}
