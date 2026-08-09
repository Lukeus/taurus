//! Turning the event stream into terminal output.
//!
//! Two shapes, because a CLI has two audiences. A person wants the model's
//! prose on stdout and its activity annotated around it; a script wants one
//! JSON object per line and nothing else on stdout that it has to filter out.

use std::io::{IsTerminal, Write};

use taurus_core::UiEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Human,
    /// Newline-delimited JSON, one event per line, on stdout.
    Json,
}

pub struct Renderer {
    format: Format,
    color: bool,
    /// Tracks whether the model is mid-sentence, so activity lines do not cut
    /// into a streaming paragraph without a break.
    mid_text: bool,
    quiet: bool,
}

impl Renderer {
    pub fn new(format: Format, quiet: bool) -> Self {
        Self {
            format,
            // Never colorize a redirected stream, and honor the de facto
            // standard opt-out.
            color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
            mid_text: false,
            quiet,
        }
    }

    pub fn handle(&mut self, event: &UiEvent) {
        match self.format {
            Format::Json => self.emit_json(event),
            Format::Human => self.emit_human(event),
        }
    }

    fn emit_json(&self, event: &UiEvent) {
        if let Ok(line) = serde_json::to_string(event) {
            println!("{line}");
        }
    }

    fn emit_human(&mut self, event: &UiEvent) {
        match event {
            UiEvent::TextDelta { text } => {
                print!("{text}");
                let _ = std::io::stdout().flush();
                self.mid_text = true;
            }

            // Reasoning is not shown by default: it is long, and on a small
            // model it is usually noise. `--verbose` surfaces it.
            UiEvent::ThinkingDelta { text } => {
                if !self.quiet {
                    self.break_text();
                    self.dim(&format!("  {}", text.trim()));
                }
            }

            UiEvent::ToolCallStarted { name, preview, .. } => {
                if self.quiet {
                    return;
                }
                self.break_text();
                self.dim(&format!("  {} {}", glyph(name), preview));
            }

            UiEvent::ToolCallFinished { ok, output, .. } => {
                if self.quiet {
                    return;
                }
                let first = output.lines().next().unwrap_or("").trim();
                let summary = if first.chars().count() > 100 {
                    format!("{}…", first.chars().take(100).collect::<String>())
                } else {
                    first.to_string()
                };
                if *ok {
                    self.dim(&format!("    ✓ {summary}"));
                } else {
                    // Failures print even in quiet mode's spirit: they explain
                    // an answer that would otherwise look unmotivated.
                    self.warn(&format!("    ✕ {summary}"));
                }
            }

            UiEvent::Compacted { messages_removed } => {
                self.break_text();
                self.dim(&format!(
                    "  [summarized {messages_removed} earlier messages to fit the context window]"
                ));
            }

            UiEvent::Error { message } => {
                self.break_text();
                self.warn(&format!("  error: {message}"));
            }

            UiEvent::IterationStarted { .. } => {}

            UiEvent::TurnFinished { usage, .. } => {
                self.break_text();
                if !self.quiet {
                    self.dim(&format!(
                        "  [{} in / {} out]",
                        usage.input_tokens, usage.output_tokens
                    ));
                }
            }
        }
    }

    /// Ends a streaming line before printing an annotation beneath it.
    fn break_text(&mut self) {
        if self.mid_text {
            println!();
            self.mid_text = false;
        }
    }

    /// Annotations go to stderr so `taurus run > answer.md` captures only the
    /// model's prose.
    fn dim(&self, line: &str) {
        let mut err = std::io::stderr();
        let _ = if self.color {
            writeln!(err, "\x1b[2m{line}\x1b[0m")
        } else {
            writeln!(err, "{line}")
        };
    }

    fn warn(&self, line: &str) {
        let mut err = std::io::stderr();
        let _ = if self.color {
            writeln!(err, "\x1b[33m{line}\x1b[0m")
        } else {
            writeln!(err, "{line}")
        };
    }

    /// Ensures the output ends on its own line.
    pub fn finish(&mut self) {
        if self.format == Format::Human {
            self.break_text();
        }
    }
}

fn glyph(tool: &str) -> &'static str {
    match tool {
        "read_file" => "read",
        "write_file" => "write",
        "edit_file" => "edit",
        "list_dir" => "ls",
        "glob" => "glob",
        "grep" => "grep",
        "run_command" => "run",
        "load_skill" => "skill",
        "run_skill_script" => "script",
        "propose_skill" => "propose",
        "spawn_subagent" => "delegate",
        _ => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_format_serializes_every_event_variant() {
        // If an event cannot round-trip, scripts consuming --json silently
        // lose it, so check the whole set rather than a sample.
        let events = vec![
            UiEvent::IterationStarted { iteration: 1 },
            UiEvent::TextDelta { text: "hi".into() },
            UiEvent::ThinkingDelta { text: "hm".into() },
            UiEvent::ToolCallStarted {
                id: "t".into(),
                name: "read_file".into(),
                preview: "Read a".into(),
            },
            UiEvent::ToolCallFinished {
                id: "t".into(),
                ok: true,
                output: "body".into(),
            },
            UiEvent::Compacted {
                messages_removed: 3,
            },
            UiEvent::Error {
                message: "boom".into(),
            },
            UiEvent::TurnFinished {
                stop_reason: taurus_provider::StopReason::EndTurn,
                usage: Default::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).expect("event must serialize");
            assert!(json.contains("\"type\""), "missing tag: {json}");
        }
    }

    #[test]
    fn every_builtin_tool_has_a_label() {
        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "glob",
            "grep",
            "run_command",
            "load_skill",
            "run_skill_script",
            "propose_skill",
            "spawn_subagent",
        ] {
            assert_ne!(glyph(tool), "tool", "{tool} fell through to the default");
        }
    }

    #[test]
    fn an_mcp_tool_falls_back_to_the_generic_label() {
        assert_eq!(glyph("mcp__github__create_issue"), "tool");
    }
}
