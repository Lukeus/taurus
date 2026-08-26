//! Turning the event stream into terminal output.
//!
//! Two shapes, because a CLI has two audiences. A person wants the model's
//! prose on stdout and its activity annotated around it; a script wants one
//! JSON object per line and nothing else on stdout that it has to filter out.

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

use taurus_core::UiEvent;
use taurus_tools::view::TranscriptView;

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
    /// Calls whose view was already printed in full.
    ///
    /// Their result says "Drew 'Crates by build time' — 5 rows", which is
    /// written for a model that cannot see the table. Echoing it under a table
    /// the reader is looking at describes the thing above it back to them.
    drawn: HashSet<String>,
    quiet: bool,
    verbose: bool,
}

/// Whether stderr can carry color.
///
/// Asked separately from the transcript renderer's decision, which is about
/// stdout, because prompts go to stderr: `taurus run > out.md` sends the answer
/// to a file while the questions stay on a terminal that can still be colored.
/// Answered once — it cannot change within a run, and the alternative was an
/// `isatty` per line of a diff.
fn stderr_color() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

fn paint(code: &str, text: &str) -> String {
    if stderr_color() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Styling for anything written to stderr, which in practice is the permission
/// prompt. With color off each returns its input unchanged, so a piped run's
/// stderr stays plain text.
pub fn dim_text(text: &str) -> String {
    paint("2", text)
}

pub fn green_text(text: &str) -> String {
    paint("32", text)
}

pub fn red_text(text: &str) -> String {
    paint("31", text)
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
            drawn: HashSet::new(),
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

            UiEvent::ToolCallStarted {
                id,
                name,
                preview,
                view,
            } => {
                if self.quiet {
                    return;
                }
                self.break_text();
                self.break_thinking();
                // A table or a chart *is* the output, so it goes to stdout in
                // full rather than being announced as a call that happened.
                // Three exceptions among the exceptions. The prompt that
                // follows draws questions itself, and printing them twice
                // would ask everything before asking anything. A dataset card
                // and a query card both point at a pane that exists only in
                // the app, and the call's own text is the better answer here —
                // so all three keep their ordinary row.
                match view {
                    Some(
                        TranscriptView::Questions { .. }
                        | TranscriptView::Dataset { .. }
                        | TranscriptView::Query { .. },
                    )
                    | None => {
                        self.dim(&format!("  {} {}", glyph(name), preview));
                    }
                    Some(view) => {
                        self.drawn.insert(id.clone());
                        println!();
                        println!("{}", crate::views::render(view, self.color));
                    }
                }
            }

            // Named, not opened. The child's transcript is a file, and
            // printing a conversation inside a conversation is the thing
            // delegation exists to avoid — but a run that is being watched
            // closely should be able to say which delegate to go and read.
            // `taurus sessions --agents <session>` is how it is listed.
            UiEvent::ToolTranscript { session, agent, .. } => {
                if self.quiet || !self.verbose {
                    return;
                }
                self.break_text();
                self.break_thinking();
                self.dim(&format!("    · {agent} transcript: {session}"));
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

            UiEvent::ToolCallFinished { id, ok, output, .. } => {
                if self.quiet || self.drawn.remove(id) {
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

            // For a UI that draws a running count somewhere the transcript is
            // not. A terminal already has the `edit_file` and `run_command`
            // lines that caused every one of these, in order, on the screen —
            // restating the total after each round would be the same news a
            // second time.
            UiEvent::FilesChanged { .. } => {}

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

            // Not drawn per iteration: this arrives before every request, and
            // a line each would be most of a transcript. The CLI reports the
            // turn's usage when it ends, which is the moment it can be read.
            UiEvent::ContextUsed { .. } => {}
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
        "check_command" => "check",
        "stop_command" => "stop",
        "web_search" => "search",
        "fetch_url" => "fetch",
        "load_skill" => "skill",
        "run_skill_script" => "script",
        "propose_skill" => "propose",
        "spawn_subagent" => "delegate",
        "search_code" => "find",
        "remember" => "note",
        // Both reach here, unlike the drawing tools: a dataset card points at a
        // pane this frontend has not got, so the call keeps its ordinary row
        // and the tool's own text does the talking.
        "load_dataset" => "load",
        "profile_dataset" => "profile",
        "query_data" => "query",
        "run_recipe" => "recipe",
        // show_table and show_chart never reach here: their view is printed in
        // full instead of announced. `ask_user` does, because the prompt that
        // follows is what draws it.
        "ask_user" => "ask",
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
                view: None,
            },
            UiEvent::ToolCallStarted {
                id: "v".into(),
                name: "show_chart".into(),
                preview: "Chart: turns".into(),
                view: Some(TranscriptView::Chart {
                    title: "turns".into(),
                    caption: None,
                    labels: vec!["t1".into()],
                    series: vec![taurus_tools::view::Series {
                        name: "calls".into(),
                        unit: String::new(),
                        values: vec![4.0],
                    }],
                }),
            },
            UiEvent::ToolCallFinished {
                id: "t".into(),
                ok: true,
                output: "body".into(),
                images: Vec::new(),
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
            "check_command",
            "stop_command",
            "web_search",
            "fetch_url",
            "load_skill",
            "run_skill_script",
            "propose_skill",
            "spawn_subagent",
            "search_code",
            "remember",
            "load_dataset",
            "profile_dataset",
            "query_data",
            "run_recipe",
        ] {
            assert_ne!(glyph(tool), "tool", "{tool} fell through to the default");
        }
    }

    #[test]
    fn an_mcp_tool_falls_back_to_the_generic_label() {
        assert_eq!(glyph("mcp__github__create_issue"), "tool");
    }
}
