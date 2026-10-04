//! `read_terminal`: what the person ran in the terminal beside the conversation.
//!
//! "Why did that fail?" typed under a red build in the dock is a complete
//! question to anyone looking at the window. Without this, it's one the model
//! has to answer by asking for a paste. The commands come from
//! [`crate::blocks`], which is where a terminal's output becomes commands at
//! all, and the tool is registered only while a dock shell is running with
//! shell integration. A tool offered with no terminal behind it would be one
//! the prompt can't explain.
//!
//! It's [`Effect::Read`], so it isn't gated. That's deliberate, and it's said
//! in the docs rather than hidden: the dock is part of the Taurus window, and
//! a person running a command there is running it where the agent works. A
//! secret typed into it is visible to the model while the dock is open, the
//! same way one pasted into the composer is.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::blocks::{BlockText, TerminalReader};
use crate::tool::{parse_input, schema_for, Effect, Tool, ToolContext, ToolError, ToolResult};

/// The tool's name, which `disabled_tools` and the per-turn set refer to.
pub const READ_TERMINAL_TOOL: &str = "read_terminal";

/// Commands listed when none is named.
const DEFAULT_LAST: usize = 5;

/// The most a listing will show.
const MAX_LAST: usize = 20;

/// Lines of each command's output a listing shows. The newest command's tail
/// is usually the answer, and the full output is one call away.
const LISTING_LINES: usize = 15;

/// The most one call hands back, in characters, kept from the end.
const MAX_READ_CHARS: usize = 16_000;

#[derive(Deserialize, JsonSchema)]
pub struct ReadTerminalInput {
    /// One command's number, from a listing, to read its whole output. Omit to
    /// list the most recent commands.
    #[serde(default)]
    pub block: Option<u64>,
    /// How many recent commands to list, newest last. Defaults to 5, at most 20.
    #[serde(default)]
    pub last: Option<usize>,
}

pub struct ReadTerminal {
    terminal: Arc<dyn TerminalReader>,
}

impl ReadTerminal {
    pub fn new(terminal: Arc<dyn TerminalReader>) -> Self {
        Self { terminal }
    }
}

#[async_trait]
impl Tool for ReadTerminal {
    fn name(&self) -> &str {
        READ_TERMINAL_TOOL
    }

    fn description(&self) -> &str {
        "Read the commands the user ran in the terminal beside this conversation: each one's \
         command line, exit status, duration, directory and output. Use it when they refer to \
         something they ran (\"why did that fail?\", \"the build is broken\"). Omit block to \
         list recent commands with the end of each one's output; pass a block number to read \
         that command's whole output. This is the user's own shell, not yours. To run a command \
         yourself, use run_command."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ReadTerminalInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        match input.get("block").and_then(|b| b.as_u64()) {
            Some(id) => format!("Read terminal command #{id}"),
            None => "List recent terminal commands".into(),
        }
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: ReadTerminalInput = parse_input(input)?;
        let shell = self.terminal.shell();
        if !self.terminal.integrated() {
            // Said rather than returned as an empty list, which would read as
            // "nothing has been run".
            return Err(ToolError::Failed(format!(
                "The terminal is open, but its shell ({shell}) isn't reporting where commands \
                 start and end, so there are no commands to read. Ask the user to paste the \
                 output instead."
            )));
        }
        if let Some(id) = input.block {
            let Some(block) = self.terminal.get(id) else {
                return Err(ToolError::InvalidInput(format!(
                    "There's no command #{id} in the terminal any more. Only the most recent \
                     {} are kept; omit block to list them.",
                    crate::blocks::MAX_BLOCKS
                )));
            };
            return Ok(taurus_provider::ToolOutput::text(whole(&block)));
        }
        let last = input.last.unwrap_or(DEFAULT_LAST).clamp(1, MAX_LAST);
        let blocks = self.terminal.recent(last);
        if blocks.is_empty() {
            return Ok(taurus_provider::ToolOutput::text(format!(
                "Nothing has been run in the terminal ({shell}) yet."
            )));
        }
        Ok(taurus_provider::ToolOutput::text(listing(&shell, &blocks)))
    }
}

/// One line about a command: its number, line, status, time and place.
pub fn heading(block: &crate::blocks::Block) -> String {
    let status = if block.running {
        "still running".to_string()
    } else {
        match block.exit {
            Some(0) => "exit 0".to_string(),
            Some(code) => format!("exit {code}"),
            None => "ended without a status".to_string(),
        }
    };
    let mut line = format!("#{} $ {}", block.id, display_command(&block.command));
    line.push_str(&format!("  ({status}"));
    if let Some(ms) = block.duration_ms {
        line.push_str(&format!(", {}", duration(ms)));
    }
    if let Some(cwd) = &block.cwd {
        line.push_str(&format!(", in {cwd}"));
    }
    line.push(')');
    line
}

fn display_command(command: &str) -> &str {
    if command.is_empty() {
        "(command line not reported)"
    } else {
        command
    }
}

fn duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}

fn listing(shell: &str, blocks: &[BlockText]) -> String {
    let mut out = format!("Recent commands in the terminal ({shell}), newest last:\n");
    for text in blocks {
        out.push('\n');
        out.push_str(&heading(&text.block));
        out.push('\n');
        out.push_str(&body(text, Some(LISTING_LINES)));
        out.push('\n');
    }
    tail_chars(&out, MAX_READ_CHARS)
}

fn whole(text: &BlockText) -> String {
    let mut out = heading(&text.block);
    out.push('\n');
    out.push_str(&body(text, None));
    tail_chars(&out, MAX_READ_CHARS)
}

/// A command's output, or a sentence saying why there isn't any.
fn body(text: &BlockText, lines: Option<usize>) -> String {
    if text.block.full_screen {
        return "(a full-screen program; what it drew isn't kept)".into();
    }
    if text.output.is_empty() {
        return "(no output)".into();
    }
    let mut out = String::new();
    if text.block.dropped_bytes > 0 {
        out.push_str(&format!(
            "[the first {} bytes of output weren't kept]\n",
            text.block.dropped_bytes
        ));
    }
    match lines {
        Some(n) => {
            let all: Vec<&str> = text.output.lines().collect();
            let skip = all.len().saturating_sub(n);
            if skip > 0 {
                out.push_str(&format!(
                    "[{skip} earlier lines; read block {} for all of it]\n",
                    text.block.id
                ));
            }
            out.push_str(&all[skip..].join("\n"));
        }
        None => out.push_str(&text.output),
    }
    out
}

/// The last `max` characters of `text`, saying so when anything went.
fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let kept: String = text.chars().skip(count - max).collect();
    format!("[{} earlier characters not shown]\n{kept}", count - max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::BlockTracker;
    use crate::test_support::test_ctx;
    use std::sync::Mutex;

    struct Fake {
        tracker: Mutex<BlockTracker>,
    }

    impl TerminalReader for Fake {
        fn shell(&self) -> String {
            "zsh".into()
        }
        fn integrated(&self) -> bool {
            self.tracker.lock().unwrap().integrated()
        }
        fn recent(&self, limit: usize) -> Vec<BlockText> {
            self.tracker.lock().unwrap().recent(limit)
        }
        fn get(&self, id: u64) -> Option<BlockText> {
            self.tracker.lock().unwrap().get(id)
        }
    }

    fn terminal(stream: &str) -> Arc<Fake> {
        let mut tracker = BlockTracker::default();
        tracker.feed(stream.as_bytes());
        Arc::new(Fake {
            tracker: Mutex::new(tracker),
        })
    }

    async fn run(fake: Arc<Fake>, input: serde_json::Value) -> Result<String, ToolError> {
        let (ctx, _dir) = test_ctx();
        ReadTerminal::new(fake)
            .execute(input, &ctx)
            .await
            .map(|out| out.as_text().expect("one text block").to_string())
    }

    const FAILED: &str = "\x1b]7;file://h/repo\x07\x1b]133;C;cmdline_url=cargo%20test\x07\
        running 2 tests\r\ntest a ... FAILED\r\n\x1b]133;D;101\x07\x1b]133;A\x07";

    #[tokio::test]
    async fn a_listing_names_each_command_with_its_status_and_output() {
        let out = run(terminal(FAILED), serde_json::json!({})).await.unwrap();
        assert!(out.contains("#1 $ cargo test  (exit 101,"), "{out}");
        assert!(out.contains("in /repo)"), "{out}");
        assert!(out.contains("test a ... FAILED"), "{out}");
    }

    #[tokio::test]
    async fn one_command_can_be_read_whole() {
        let mut stream = String::from("\x1b]133;C;cmdline_url=seq\x07");
        for n in 0..100 {
            stream.push_str(&format!("line {n}\r\n"));
        }
        stream.push_str("\x1b]133;D;0\x07");
        let fake = terminal(&stream);
        let listed = run(fake.clone(), serde_json::json!({})).await.unwrap();
        assert!(
            listed.contains("[85 earlier lines; read block 1"),
            "{listed}"
        );
        assert!(!listed.contains("line 0\n"));
        let whole = run(fake, serde_json::json!({ "block": 1 })).await.unwrap();
        assert!(
            whole.contains("line 0\n") && whole.contains("line 99"),
            "{whole}"
        );
    }

    #[tokio::test]
    async fn a_shell_without_integration_is_said_plainly() {
        let err = run(terminal("$ ls\r\nfile\r\n"), serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("isn't reporting where commands"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_command_no_longer_kept_says_how_to_find_the_rest() {
        let err = run(terminal(FAILED), serde_json::json!({ "block": 9 }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("omit block to list them"), "{err}");
    }

    #[test]
    fn durations_read_the_way_a_person_says_them() {
        assert_eq!(duration(850), "850ms");
        assert_eq!(duration(4_200), "4.2s");
        assert_eq!(duration(125_000), "2m05s");
    }
}
