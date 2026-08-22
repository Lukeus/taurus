//! `taurus usage` — where a session's context window actually went.
//!
//! The token counter in the header answers "how much", which is the question
//! you ask once. This answers "on what", which is the one you can act on: a
//! tool that reads whole files when it wanted three lines, a grep run four
//! times with the same pattern, a transcript whose bulk is one build log.
//!
//! Everything here is read back out of the transcript rather than tracked
//! alongside it, for the reason the transcript format already gives: a second
//! copy of the truth is a copy that can disagree with it. The cost is that the
//! per-tool figures are estimates — the provider reports one number for a whole
//! request and never says which part of the prompt was whose.

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

use taurus_core::{estimate_block, estimate_message, estimate_tokens, Session};
use taurus_host::sessions;
use taurus_provider::{ContentBlock, Role, ToolDef};

/// How many tool schemas to name individually before summarizing the rest.
const SCHEMAS_LISTED: usize = 5;

pub fn run(
    workspace: &Path,
    id: Option<&str>,
    all: bool,
    fixed: &Fixed,
) -> Result<ExitCode, String> {
    if all {
        let listed = sessions::list(Some(workspace));
        if listed.is_empty() {
            return Ok(nothing_recorded(workspace, fixed));
        }
        let mut report = Report::default();
        let mut counted = 0;
        for meta in &listed {
            if let Ok(loaded) = sessions::load(&meta.id) {
                report.absorb(&loaded.session);
                counted += 1;
            }
        }
        println!("{counted} sessions in {}\n", workspace.display());
        report.print();
        fixed.print();
        return Ok(ExitCode::SUCCESS);
    }

    let id = match id {
        Some(id) => id.to_string(),
        // A named session that does not exist is a mistake worth an error. No
        // sessions at all is not: half of what this command reports comes from
        // the configuration rather than a transcript, and it is the half you
        // would want before starting rather than after.
        None => match sessions::latest(workspace) {
            Some(meta) => meta.id,
            None => return Ok(nothing_recorded(workspace, fixed)),
        },
    };

    let session = sessions::load(&id)?.session;
    println!("Session {}\nModel   {}\n", session.id, session.model);
    let mut report = Report::default();
    report.absorb(&session);
    report.print();
    fixed.print();
    Ok(ExitCode::SUCCESS)
}

/// What there is to say about a workspace nothing has been run in yet.
fn nothing_recorded(workspace: &Path, fixed: &Fixed) -> ExitCode {
    println!(
        "No saved sessions for {}, so there is no history to account for.",
        workspace.display()
    );
    fixed.print();
    ExitCode::SUCCESS
}

/// The part of every request that is not the conversation.
///
/// History is there because the turn needed it. This is not: the system prompt
/// and every tool's schema go out again on each iteration, called or not, and
/// they are the reason a transcript worth a thousand tokens can bill twenty.
/// Reported next to the session totals because that gap is what sends people
/// looking, and the answer is almost never in the transcript.
pub struct Fixed {
    pub system_prompt: u32,
    pub tools: Vec<ToolDef>,
}

impl Fixed {
    pub fn new(system_prompt: &str, tools: Vec<ToolDef>) -> Self {
        Self {
            system_prompt: estimate_tokens(system_prompt),
            tools,
        }
    }

    /// What advertising one tool costs: its name, its description, and its
    /// schema, which is what the provider is handed.
    fn cost(def: &ToolDef) -> u32 {
        estimate_tokens(&def.name)
            + estimate_tokens(&def.description)
            + estimate_tokens(&def.input_schema.to_string())
    }

    fn print(&self) {
        let mut costs: Vec<(&str, u32)> = self
            .tools
            .iter()
            .map(|d| (d.name.as_str(), Self::cost(d)))
            .collect();
        let schemas: u32 = costs.iter().map(|(_, c)| c).sum();

        println!(
            "\nSent again with every request  ~{} tokens",
            thousands(self.system_prompt + schemas)
        );
        println!(
            "  {:<26} {:>6}",
            "system prompt",
            thousands(self.system_prompt)
        );
        println!(
            "  {:<26} {:>6}",
            format!("{} tool schemas", self.tools.len()),
            thousands(schemas)
        );

        if costs.is_empty() {
            return;
        }
        costs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        println!("\nHeaviest tool schemas");
        for (name, cost) in costs.iter().take(SCHEMAS_LISTED) {
            println!("  {name:<26} {:>6}", thousands(*cost));
        }
        // Said rather than left implied: a list that stops without saying so
        // reads as the whole list, and the tools it hid are exactly the ones
        // somebody deciding what to turn off would want to know about.
        if let Some(rest) = costs.len().checked_sub(SCHEMAS_LISTED).filter(|n| *n > 0) {
            let hidden: u32 = costs.iter().skip(SCHEMAS_LISTED).map(|(_, c)| c).sum();
            println!("  {:<26} {:>6}", format!("{rest} more"), thousands(hidden));
        }
        println!(
            "\nTurn off what this workspace does not use with `disabled_tools` in settings.json."
        );
    }
}

/// What one tool cost, across every call to it.
#[derive(Default)]
struct ToolCost {
    calls: u32,
    /// Both directions: the arguments sent and the output returned. A
    /// `write_file` carries its whole payload in the call and answers in a
    /// line, so counting only results would report it as free.
    tokens: u32,
    failures: u32,
}

#[derive(Default)]
struct Report {
    turns: u32,
    messages: usize,
    /// What the provider actually billed, summed over the sessions read.
    reported_in: u32,
    reported_out: u32,
    /// Of `reported_in`, what came from the provider's prompt cache.
    ///
    /// `None` until a backend reports one, and shown only then. A local Ollama
    /// has no cache to have missed, and a line reading `0 cached` beside its
    /// numbers would invite exactly the wrong conclusion.
    cached_in: Option<u32>,
    /// What the transcript holds now, estimated.
    history: u32,
    by_tool: HashMap<String, ToolCost>,
    /// Calls that repeated an earlier call exactly, and what those repeats
    /// cost. The most actionable number here: it is pure waste.
    repeats: u32,
    repeat_tokens: u32,
}

impl Report {
    fn absorb(&mut self, session: &Session) {
        self.reported_in += session.usage.input_tokens;
        self.reported_out += session.usage.output_tokens;
        if let Some(cached) = session.usage.cache_read_input_tokens {
            *self.cached_in.get_or_insert(0) += cached;
        }
        self.messages += session.messages.len();

        // Tool name and cost, keyed by the call id its result will carry.
        let mut pending: HashMap<&str, (&str, u32)> = HashMap::new();
        let mut seen: HashMap<String, u32> = HashMap::new();

        for message in &session.messages {
            // The whole message, envelope included, so this total is the one
            // the compaction trigger is working from and not four tokens a
            // message off it.
            self.history += estimate_message(message);

            // A user message that is not a tool result is somebody typing.
            if message.role == Role::User
                && !message
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            {
                self.turns += 1;
            }

            for block in &message.content {
                match block {
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    } => {
                        let cost = estimate_block(block);
                        let entry = self.by_tool.entry(name.to_string()).or_default();
                        entry.calls += 1;
                        entry.tokens += cost;
                        pending.insert(id, (name, cost));

                        let signature = format!("{name}\u{0}{input}");
                        let previous = seen.entry(signature).or_default();
                        *previous += 1;
                        if *previous > 1 {
                            self.repeats += 1;
                            self.repeat_tokens += cost;
                        }
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        is_error,
                        ..
                    } => {
                        // A result whose call is missing belongs to a
                        // conversation that was compacted out from under it.
                        let Some((name, _)) = pending.get(tool_use_id.as_str()) else {
                            continue;
                        };
                        let entry = self.by_tool.entry(name.to_string()).or_default();
                        entry.tokens += estimate_block(block);
                        if *is_error {
                            entry.failures += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn print(&self) {
        println!(
            "Turns              {}\nMessages           {}",
            self.turns, self.messages
        );
        println!(
            "Billed by provider {} in / {} out",
            thousands(self.reported_in),
            thousands(self.reported_out)
        );
        // Only when there was a cache to read from. The split is the difference
        // between a bill somebody can explain and a number that just went up:
        // a cached input token costs about a tenth of a fresh one.
        if let Some(cached) = self.cached_in.filter(|c| *c > 0) {
            let share = (cached as f64 / self.reported_in.max(1) as f64) * 100.0;
            println!(
                "  of which cached  {} ({share:.0}% of input)",
                thousands(cached)
            );
        }
        println!("Transcript holds   ~{} tokens\n", thousands(self.history));

        if self.by_tool.is_empty() {
            println!("No tool calls recorded.");
            return;
        }

        let mut rows: Vec<(&String, &ToolCost)> = self.by_tool.iter().collect();
        rows.sort_by(|a, b| b.1.tokens.cmp(&a.1.tokens).then(a.0.cmp(b.0)));
        let total: u32 = rows.iter().map(|(_, c)| c.tokens).sum();

        println!(
            "{:<22} {:>6} {:>10} {:>7}",
            "Tool", "calls", "~tokens", "share"
        );
        for (name, cost) in rows {
            let share = if total == 0 {
                0
            } else {
                (cost.tokens as u64 * 100 / total as u64) as u32
            };
            let failures = if cost.failures > 0 {
                format!("   {} failed", cost.failures)
            } else {
                String::new()
            };
            println!(
                "{name:<22} {:>6} {:>10} {:>6}%{failures}",
                cost.calls,
                thousands(cost.tokens),
                share
            );
        }

        if self.repeats > 0 {
            println!(
                "\n{} of those calls repeated an earlier one exactly (~{} tokens). Same tool, \
                 same input.",
                self.repeats,
                thousands(self.repeat_tokens)
            );
        }
    }
}

/// `1234567` as `1,234,567`. Token counts are read, not computed with, and the
/// difference between 40k and 400k is the whole point of printing them.
fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use taurus_provider::Message;

    fn call(id: &str, name: &str, path: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(id, name, json!({ "path": path }))],
        )
    }

    fn result(id: &str, body: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::tool_result(id, body)])
    }

    fn session_with(messages: Vec<Message>) -> Session {
        let mut session = Session::new("m");
        session.messages = messages;
        session
    }

    #[test]
    fn a_tools_cost_counts_both_its_arguments_and_its_output() {
        let mut report = Report::default();
        report.absorb(&session_with(vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &"x".repeat(4000)),
        ]));

        let cost = &report.by_tool["read_file"];
        assert_eq!(cost.calls, 1);
        assert!(cost.tokens >= 1000, "{} tokens", cost.tokens);
    }

    #[test]
    fn an_identical_call_is_counted_as_a_repeat() {
        let mut report = Report::default();
        report.absorb(&session_with(vec![
            call("t1", "grep", "TODO"),
            result("t1", "hit"),
            call("t2", "grep", "TODO"),
            result("t2", "hit"),
        ]));

        assert_eq!(report.repeats, 1);
        assert_eq!(report.by_tool["grep"].calls, 2);
    }

    #[test]
    fn a_different_input_is_not_a_repeat() {
        let mut report = Report::default();
        report.absorb(&session_with(vec![
            call("t1", "grep", "TODO"),
            call("t2", "grep", "FIXME"),
        ]));
        assert_eq!(report.repeats, 0);
    }

    #[test]
    fn a_failed_call_is_counted_as_one() {
        let mut report = Report::default();
        report.absorb(&session_with(vec![
            call("t1", "shell", "x"),
            Message::new(Role::User, vec![ContentBlock::tool_error("t1", "boom")]),
        ]));
        assert_eq!(report.by_tool["shell"].failures, 1);
    }

    #[test]
    fn typed_messages_are_turns_and_tool_results_are_not() {
        let mut report = Report::default();
        report.absorb(&session_with(vec![
            Message::user("do a thing"),
            call("t1", "shell", "x"),
            result("t1", "ok"),
            Message::assistant("done"),
            Message::user("and another"),
        ]));
        assert_eq!(report.turns, 2);
    }

    #[test]
    fn absorbing_two_sessions_adds_them_up_without_inventing_repeats() {
        let mut report = Report::default();
        report.absorb(&session_with(vec![call("t1", "grep", "a")]));
        report.absorb(&session_with(vec![call("t1", "grep", "a")]));

        assert_eq!(report.by_tool["grep"].calls, 2);
        // Two conversations that each asked once. Only a call repeated inside
        // one conversation is work that was already done.
        assert_eq!(report.repeats, 0);
    }

    #[test]
    fn thousands_separates_every_three_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
