//! Shell execution.
//!
//! Commands run non-interactively with stdin closed. A model cannot answer a
//! `[y/N]` prompt, so a command that waits for one must fail on the timeout
//! rather than hang the session forever.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::process::Command;

use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Deserialize, JsonSchema)]
pub struct RunCommandInput {
    /// The command line to run, as you would type it in a terminal.
    pub command: String,
    /// Working directory, relative to the workspace root. Defaults to the root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Seconds before the command is killed. Defaults to 120, maximum 600.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub struct RunCommand;

#[async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Run a shell command and return its output. It starts in the workspace root, so write \
         paths relative to it and do not cd first — use the cwd argument for a subdirectory. Runs \
         non-interactively with no stdin, so pass flags like -y rather than expecting a prompt. \
         Prefer read_file, glob, and grep over cat, find, and grep -r."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<RunCommandInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn touches_unpredictably(&self) -> bool {
        true
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let command = input
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("?")
            .trim();
        // The user is approving this exact command line, so show it in full up
        // to a sane width rather than an elided summary.
        let shown: String = command.chars().take(300).collect();
        format!("Run: {shown}")
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: RunCommandInput = parse_input(input)?;
        if input.command.trim().is_empty() {
            return Err(ToolError::InvalidInput("command must not be empty".into()));
        }

        let cwd = ctx.resolve(input.cwd.as_deref().unwrap_or("."))?;
        let timeout = Duration::from_secs(
            input
                .timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        );

        let (program, args) = shell_invocation(&input.command);
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Without this the child outlives a canceled turn and keeps writing.
        command.kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| ToolError::Failed(format!("cannot start shell: {e}")))?;

        // Drain the pipes concurrently with the wait. A child that fills its
        // stdout buffer blocks forever if nobody is reading, which would turn
        // every chatty command into a timeout.
        let drain_stdout = spawn_drain(child.stdout.take());
        let drain_stderr = spawn_drain(child.stderr.take());

        let status = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                let _ = child.start_kill();
                return Err(ToolError::Canceled);
            }
            result = tokio::time::timeout(timeout, child.wait()) => result,
        };

        let status = match status {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(ToolError::Failed(e.to_string())),
            Err(_) => {
                let _ = child.start_kill();
                return Err(ToolError::Failed(format!(
                    "Command timed out after {}s and was killed. If it needs longer, raise \
                     timeout_secs; if it was waiting for input, rerun it non-interactively.",
                    timeout.as_secs()
                )));
            }
        };

        let code = status.code();
        let stdout = truncate(&drain_stdout.await);
        let stderr = truncate(&drain_stderr.await);

        let mut report = String::new();
        if !stdout.trim().is_empty() {
            report.push_str(&stdout);
        }
        if !stderr.trim().is_empty() {
            if !report.is_empty() {
                report.push('\n');
            }
            report.push_str("[stderr]\n");
            report.push_str(&stderr);
        }
        if report.trim().is_empty() {
            report.push_str("(no output)");
        }

        // A non-zero exit is information for the model, not a harness failure:
        // returning it as Ok lets the model read the compiler errors it just
        // asked for instead of seeing a bare error string.
        match code {
            Some(0) => Ok(report),
            Some(code) => Ok(format!("Exit code {code}\n{report}")),
            None => Ok(format!("Killed by signal\n{report}")),
        }
    }
}

/// Reads a child pipe to end in the background, yielding its text.
fn spawn_drain<R>(pipe: Option<R>) -> impl std::future::Future<Output = String>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            use tokio::io::AsyncReadExt;
            let _ = pipe.read_to_end(&mut buf).await;
        }
        String::from_utf8_lossy(&buf).into_owned()
    });
    async move { handle.await.unwrap_or_default() }
}

/// The shell to run a command line through on this platform.
#[cfg(windows)]
fn shell_invocation(command: &str) -> (String, Vec<String>) {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    (shell, vec!["/C".into(), command.to_string()])
}

#[cfg(not(windows))]
fn shell_invocation(command: &str) -> (String, Vec<String>) {
    // Deliberately /bin/sh rather than $SHELL: the model writes POSIX command
    // lines, and a user's fish or nushell login shell would not parse them.
    ("/bin/sh".into(), vec!["-c".into(), command.to_string()])
}

fn truncate(text: &str) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text.to_string();
    }
    // Keep the tail as well as the head: errors and summaries live at the end.
    let head_len = MAX_OUTPUT_BYTES * 2 / 3;
    let head = floor_boundary(text, head_len);
    let tail_start = text.len() - (MAX_OUTPUT_BYTES - head_len);
    let tail = ceil_boundary(text, tail_start);
    format!(
        "{}\n\n[… {} bytes omitted …]\n\n{}",
        &text[..head],
        text.len() - MAX_OUTPUT_BYTES,
        &text[tail..]
    )
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_ctx;

    #[tokio::test]
    async fn captures_stdout() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(serde_json::json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("hello"));
    }

    #[tokio::test]
    async fn runs_in_the_workspace_by_default() {
        let (ctx, dir) = test_ctx();
        std::fs::write(dir.path().join("marker.txt"), "").unwrap();
        let out = RunCommand
            .execute(serde_json::json!({"command": "ls"}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("marker.txt"));
    }

    // `shell_invocation` runs the platform's own shell -- /bin/sh elsewhere,
    // cmd.exe on Windows -- so a test that exercises shell *syntax* has to
    // speak the right dialect. The behavior under test is identical on both.

    /// Writes to stderr, then exits non-zero.
    #[cfg(windows)]
    const FAILS_WITH_STDERR: &str = "echo oops 1>&2 & exit 3";
    #[cfg(not(windows))]
    const FAILS_WITH_STDERR: &str = "echo oops >&2; exit 3";

    /// Blocks on a line of stdin, then reports that it got past the read.
    #[cfg(windows)]
    const READS_STDIN: &str = "set /p line= & echo got:";
    #[cfg(not(windows))]
    const READS_STDIN: &str = "read line; echo \"got:$line\"";

    #[tokio::test]
    async fn a_failing_command_returns_its_output_not_an_error() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(serde_json::json!({"command": FAILS_WITH_STDERR}), &ctx)
            .await
            .unwrap();
        assert!(out.contains("Exit code 3"));
        assert!(out.contains("oops"));
    }

    #[tokio::test]
    async fn silent_success_is_reported_explicitly() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(serde_json::json!({"command": "true"}), &ctx)
            .await
            .unwrap();
        assert_eq!(out, "(no output)");
    }

    #[tokio::test]
    async fn a_hanging_command_is_killed_by_the_timeout() {
        let (ctx, _dir) = test_ctx();
        let err = RunCommand
            .execute(
                serde_json::json!({"command": "sleep 30", "timeout_secs": 1}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn stdin_is_closed_so_interactive_reads_do_not_hang() {
        let (ctx, _dir) = test_ctx();
        let out = RunCommand
            .execute(
                serde_json::json!({"command": READS_STDIN, "timeout_secs": 5}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.contains("got:"));
    }

    #[tokio::test]
    async fn cancellation_stops_the_command() {
        let (ctx, _dir) = test_ctx();
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let err = RunCommand
            .execute(
                serde_json::json!({"command": "sleep 30", "timeout_secs": 60}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Canceled));
    }

    #[tokio::test]
    async fn rejects_an_empty_command() {
        let (ctx, _dir) = test_ctx();
        assert!(matches!(
            RunCommand
                .execute(serde_json::json!({"command": "   "}), &ctx)
                .await,
            Err(ToolError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn cwd_outside_the_workspace_is_refused() {
        let (ctx, _dir) = test_ctx();
        let err = RunCommand
            .execute(serde_json::json!({"command": "ls", "cwd": "/etc"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::OutsideWorkspace { .. }));
    }

    #[test]
    fn truncation_keeps_both_ends() {
        let text = format!("HEAD{}TAIL", "x".repeat(MAX_OUTPUT_BYTES * 2));
        let out = truncate(&text);
        assert!(out.starts_with("HEAD"));
        assert!(out.ends_with("TAIL"));
        assert!(out.contains("bytes omitted"));
    }
}
