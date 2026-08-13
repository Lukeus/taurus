//! Turning the event stream into terminal output.
//!
//! Two shapes, because a CLI has two audiences. A person wants the model's
//! prose on stdout and its activity annotated around it; a script wants one
//! JSON object per line and nothing else on stdout that it has to filter out.

use std::io::{IsTerminal, Write};

use taurus_core::UiEvent;

use crate::markdown::MarkdownStyler;

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
    /// Styling is line-oriented, so text is held here until a newline arrives.
    /// The cost is that prose appears a line at a time rather than a token at
    /// a time; the alternative is redrawing the current line with cursor
    /// escapes, which corrupts output the moment a line wraps.
    pending: String,
    styler: MarkdownStyler,
    /// Same, for reasoning, which streams to stderr on its own line.
    mid_thinking: bool,
    quiet: bool,
    verbose: bool,
}

impl Renderer {
    pub fn new(format: Format, quiet: bool, verbose: bool) -> Self {
        // Never colorize a redirected stream, and honor the de facto standard
        // opt-out. With color off the styler passes markdown through untouched.
        let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Self {
            format,
            color,
            mid_text: false,
            mid_thinking: false,
            pending: String::new(),
            styler: MarkdownStyler::new(color),
            quiet,
            verbose,
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
                self.pending.push_str(text);
                while let Some(at) = self.pending.find('\n') {
                    let line: String = self.pending.drain(..=at).collect();
                    let styled = self.styler.line(line.trim_end_matches('\n'));
                    println!("{styled}");
                }
                self.mid_text = !self.pending.is_empty();
            }

            // Reasoning is hidden unless asked for: it is long, and on a small
            // model it is mostly noise. When shown it must stream inline —
            // these deltas are token-sized, so a line each would print one
            // word per line.
            UiEvent::ThinkingDelta { text } => {
                if !self.verbose {
                    return;
                }
                self.break_text();
                if !self.mid_thinking {
                    self.write_err("  ");
                    self.mid_thinking = true;
                }
                self.write_err(text);
            }

            UiEvent::ToolCallStarted { name, preview, .. } => {
                if self.quiet {
                    return;
                }
                self.break_text();
                self.break_thinking();
                self.dim(&format!("  {} {}", glyph(name), preview));
            }

            // Indented under the call it belongs to. Delegation is the only
            // thing that reports these, and watching the child work is the
            // difference between a minute of silence and a minute of progress.
            UiEvent::ToolProgress { label, .. } => {
                if self.quiet {
                    return;
                }
                self.break_text();
                self.break_thinking();
                self.dim(&format!("    · {label}"));
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

            // Warn rather than dim: this is the one notice that explains a wait
            // the user is currently sitting through, so it has to survive the
            // same glance a dimmed line would not.
            UiEvent::Retrying {
                attempt,
                of,
                reason,
            } => {
                self.break_text();
                self.break_thinking();
                self.warn(&format!("  retrying ({attempt}/{of}): {reason}"));
            }

            UiEvent::ContextTrimmed {
                results,
                tokens_saved,
            } => {
                self.break_text();
                self.break_thinking();
                self.dim(&format!(
                    "  [shortened {results} older tool results, ~{tokens_saved} tokens]"
                ));
            }

            UiEvent::Compacted { messages_removed } => {
                self.break_text();
                self.break_thinking();
                self.dim(&format!(
                    "  [summarized {messages_removed} earlier messages to fit the context window]"
                ));
            }

            UiEvent::Error { message } => {
                self.break_text();
                self.break_thinking();
                self.warn(&format!("  error: {message}"));
            }

            UiEvent::IterationStarted { .. } => {}

            UiEvent::TurnFinished { usage, .. } => {
                self.break_text();
                self.break_thinking();
                if !self.quiet {
                    self.dim(&format!(
                        "  [{} in / {} out]",
                        usage.input_tokens, usage.output_tokens
                    ));
                }
            }
        }
    }

    /// Emits any buffered partial line before printing an annotation beneath
    /// it, so a half-written sentence is never left hanging.
    fn break_text(&mut self) {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            let styled = self.styler.partial(&line);
            println!("{styled}");
        } else if self.mid_text {
            println!();
        }
        self.mid_text = false;
    }

    fn break_thinking(&mut self) {
        if self.mid_thinking {
            let _ = writeln!(std::io::stderr());
            self.mid_thinking = false;
        }
    }

    /// Dimmed, no trailing newline — for streaming reasoning.
    fn write_err(&self, text: &str) {
        let mut err = std::io::stderr();
        let _ = if self.color {
            write!(err, "\x1b[2m{text}\x1b[0m")
        } else {
            write!(err, "{text}")
        };
        let _ = err.flush();
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
            self.break_thinking();
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
        "web_search" => "search",
        "fetch_url" => "fetch",
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
            "web_search",
            "fetch_url",
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
