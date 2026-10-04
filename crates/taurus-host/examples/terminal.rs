//! Your own shell, marked, and a model asked about what you ran in it.
//!
//! `cargo run -p taurus-host --example terminal -- <model> [shell]`
//!
//! Starts your login shell (or `shell`) the way the terminal dock does, with
//! Taurus's integration and your real startup files, so a prompt framework or
//! plugin that fights the hooks shows up here. Runs two commands in it, one
//! that fails, and prints the blocks they became. Then gives a model
//! `read_terminal` over those blocks and asks why the last command failed.
//!
//! Asserted: both commands come back as blocks with the right status and
//! output, and the model calls `read_terminal`. Printed, not asserted: its
//! answer, because what a model makes of a failure is a measurement, not a gate.
//!
//! Needs Ollama, and zsh or bash. It writes only inside temporary directories
//! (the scripts go in one, and the shell runs in another).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use taurus_core::{Agent, AgentConfig, Session};
use taurus_provider::Message;
use taurus_provider_ollama::{OllamaProvider, DEFAULT_BASE_URL};
use taurus_tools::blocks::{BlockText, BlockTracker, TerminalReader};
use taurus_tools::builtin::terminal::{heading, ReadTerminal};
use taurus_tools::{AllowAll, PermissionEngine, ToolContext, ToolRegistry};
use tokio_util::sync::CancellationToken;

const MISSING: &str = "no-such-file-here.txt";

struct Shell {
    name: String,
    blocks: Mutex<BlockTracker>,
}

impl TerminalReader for Shell {
    fn shell(&self) -> String {
        self.name.clone()
    }
    fn integrated(&self) -> bool {
        self.blocks.lock().unwrap().integrated()
    }
    fn recent(&self, limit: usize) -> Vec<BlockText> {
        self.blocks.lock().unwrap().recent(limit)
    }
    fn get(&self, id: u64) -> Option<BlockText> {
        self.blocks.lock().unwrap().get(id)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .ok_or("usage: terminal <model> [shell]")?;
    let program = std::env::args()
        .nth(2)
        .unwrap_or_else(|| CommandBuilder::new_default_prog().get_shell());
    let name = taurus_tools::shell_integration::name_of(&program);

    let scripts = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let mut builder = taurus_tools::shell_integration::command(&program, scripts.path())
        .ok_or_else(|| format!("{program} isn't a shell Taurus integrates (zsh or bash)"))?;
    builder.cwd(workspace.path());
    builder.env("TERM", "xterm-256color");

    println!("{name}: your startup files, plus Taurus's hooks\n");
    let shell = Arc::new(Shell {
        name,
        blocks: Mutex::new(BlockTracker::default()),
    });
    drive(
        builder,
        &shell,
        &["echo hello from the dock", &format!("cat {MISSING}")],
    )?;

    let blocks = shell.recent(10);
    for block in &blocks {
        println!("{}", heading(&block.block));
        println!("  {}\n", block.output.replace('\n', "\n  "));
    }
    let hello = blocks
        .iter()
        .find(|b| b.block.command == "echo hello from the dock")
        .ok_or(
            "the first command didn't become a block; your startup files may replace the hooks",
        )?;
    assert_eq!(hello.block.exit, Some(0));
    assert_eq!(hello.output, "hello from the dock");
    let failed = blocks
        .iter()
        .find(|b| b.block.command.starts_with("cat "))
        .ok_or("the failing command didn't become a block")?;
    assert_ne!(
        failed.block.exit,
        Some(0),
        "cat of a missing file exits non-zero"
    );
    assert!(failed.output.contains(MISSING), "{}", failed.output);

    println!("asking {model} why the last command failed…\n");
    let ctx = ToolContext::new(
        workspace.path().to_path_buf(),
        Arc::new(PermissionEngine::new(
            workspace.path(),
            workspace.path().join(".taurus"),
            Box::new(AllowAll),
        )),
        CancellationToken::new(),
    );
    let mut registry = ToolRegistry::with_builtins();
    registry.register(Arc::new(ReadTerminal::new(shell.clone())));
    let provider = Arc::new(OllamaProvider::new(DEFAULT_BASE_URL.to_string()));
    let agent = Agent::new(provider, registry, ctx, AgentConfig::default());
    let mut session = Session::new(&model);
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let started = Instant::now();
    let outcome = agent
        .run_turn(
            &mut session,
            Message::user("The last command I ran in my terminal failed. Why?"),
            tx,
        )
        .await;
    drain.await?;
    outcome.map_err(|e| format!("the turn failed: {e}"))?;

    let called = session
        .messages
        .iter()
        .any(|m| m.tool_uses().any(|(_, name, _)| name == "read_terminal"));
    let answer = session
        .messages
        .last()
        .map(|m| m.text())
        .unwrap_or_default();
    println!("{answer}\n");
    println!(
        "read_terminal called: {called} · {:.1}s",
        started.elapsed().as_secs_f32()
    );
    assert!(called, "the model answered without reading the terminal");
    Ok(())
}

/// Runs `lines` in a real shell started from `builder`, one after each prompt,
/// feeding everything it prints to `shell`'s tracker.
fn drive(
    builder: CommandBuilder,
    shell: &Shell,
    lines: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 30,
        cols: 160,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut child = pair.slave.spawn_command(builder)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut queue = lines
        .iter()
        .map(|l| l.to_string())
        .chain(["exit".to_string()]);
    let mut seen = Vec::new();
    let mut prompts = 0;
    // A prompt framework can take a while on its first draw.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                seen.extend_from_slice(&chunk);
                shell.blocks.lock().unwrap().feed(&chunk);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let drawn = count(&seen, b"\x1b]133;B");
        if drawn > prompts {
            prompts = drawn;
            if let Some(line) = queue.next() {
                writer.write_all(format!("{line}\n").as_bytes())?;
                writer.flush()?;
            }
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err(format!(
                "the shell drew {prompts} marked prompts in 30s and stopped; what it printed:\n{}",
                String::from_utf8_lossy(&seen)
            )
            .into());
        }
    }
    let _ = child.wait();
    // The dock does the same when a shell's output ends: whatever was
    // running (here, `exit`) has ended with it.
    shell.blocks.lock().unwrap().close();
    Ok(())
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}
