//! Where a session's context window actually went.
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
//!
//! It lives in the host rather than in either frontend because both of them ask
//! it. `taurus usage` prints it and the desktop app draws it, and the one thing
//! that must not happen is the two of them disagreeing about what a tool cost —
//! so the arithmetic, the ordering, and the shares are all decided here and
//! what crosses either boundary is a finished report.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use taurus_core::{estimate_block, estimate_message, estimate_tokens, Session};
use taurus_provider::{ContentBlock, Role, ToolDef};

use crate::sessions;

/// What one tool cost, across every call to it in what was read.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolUsage {
    pub name: String,
    pub calls: u32,
    /// Both directions: the arguments sent and the output returned. A
    /// `write_file` carries its whole payload in the call and answers in a
    /// line, so counting only results would report it as free.
    pub tokens: u32,
    pub failures: u32,
    /// Whole percent of all tool tokens. Computed once, here, so a bar drawn
    /// from it and a column printed from it cannot round differently.
    pub share: u32,
}

/// What advertising one tool costs on every request, called or not.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SchemaCost {
    pub name: String,
    pub tokens: u32,
}

/// The whole account, ready to print or to draw.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageReport {
    /// How many transcripts were read. One, unless the whole workspace was
    /// asked for — and worth carrying either way, because "no tool calls
    /// recorded" means something different across forty sessions than across
    /// one.
    pub sessions: u32,
    pub turns: u32,
    pub messages: u32,
    /// What the provider actually billed, summed over the sessions read.
    pub reported_in: u32,
    pub reported_out: u32,
    /// Of `reported_in`, what came from the provider's prompt cache.
    ///
    /// `None` until a backend reports one, and shown only then. A local Ollama
    /// has no cache to have missed, and a line reading `0 cached` beside its
    /// numbers would invite exactly the wrong conclusion.
    pub cached_in: Option<u32>,
    /// What the transcript holds now, estimated.
    pub history: u32,
    /// Heaviest first, then by name so the order is stable between runs.
    pub tools: Vec<ToolUsage>,
    /// Calls that repeated an earlier call exactly, and what those repeats
    /// cost. The most actionable number here: it is pure waste.
    pub repeats: u32,
    pub repeat_tokens: u32,
    /// The part of every request that is not the conversation. History is
    /// there because the turn needed it; this is not.
    pub system_prompt: u32,
    /// Every advertised tool, heaviest schema first. Whoever draws this
    /// decides how many to name — the cut is a rendering question, and a list
    /// truncated before it crossed the boundary could not be uncut.
    pub schemas: Vec<SchemaCost>,
}

impl UsageReport {
    /// What the tool schemas cost together.
    pub fn schema_tokens(&self) -> u32 {
        self.schemas.iter().map(|s| s.tokens).sum()
    }

    /// What goes out again on every single request.
    pub fn fixed_tokens(&self) -> u32 {
        self.system_prompt + self.schema_tokens()
    }

    /// Whether any transcript was found at all.
    ///
    /// Worth its own question rather than `sessions == 0` at each call site:
    /// half of what this reports comes from the configuration rather than from
    /// a transcript, and that half is the part you would want *before* running
    /// anything. So an empty workspace still gets a report; it just has
    /// nothing in the left-hand column.
    pub fn is_empty(&self) -> bool {
        self.sessions == 0
    }
}

/// The system prompt and tool schemas a session would be run with.
///
/// Separate from the transcript because it is not in the transcript: these go
/// out again on each iteration, called or not, and they are the reason a
/// conversation worth a thousand tokens can bill twenty. Taken from the live
/// host, so what is reported is what the *next* request will cost rather than
/// what some earlier one did.
pub struct Fixed {
    system_prompt: u32,
    schemas: Vec<SchemaCost>,
}

impl Fixed {
    pub fn new(system_prompt: &str, tools: Vec<ToolDef>) -> Self {
        let mut schemas: Vec<SchemaCost> = tools
            .iter()
            .map(|def| SchemaCost {
                name: def.name.clone(),
                // Name, description, and schema: what the provider is handed.
                tokens: estimate_tokens(&def.name)
                    + estimate_tokens(&def.description)
                    + estimate_tokens(&def.input_schema.to_string()),
            })
            .collect();
        schemas.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.name.cmp(&b.name)));
        Self {
            system_prompt: estimate_tokens(system_prompt),
            schemas,
        }
    }
}

/// The account for one session, or for every session in a workspace.
///
/// `id` names a session; `None` means the most recent one in the workspace.
/// Asking for a session that does not exist is an error, because it is a typo.
/// Finding no sessions at all is not — see [`UsageReport::is_empty`].
pub fn report(
    workspace: &Path,
    id: Option<&str>,
    all: bool,
    fixed: &Fixed,
) -> Result<UsageReport, String> {
    let mut tally = Tally::default();

    if all {
        for meta in sessions::list(Some(workspace)) {
            // A transcript that will not load is skipped rather than fatal:
            // one unreadable file in a workspace of forty should not cost the
            // answer about the other thirty-nine. It is left out of the count
            // as well, so the sessions figure means "read" and not "found".
            if let Ok(loaded) = sessions::load(&meta.id) {
                tally.absorb(&loaded.session);
            }
        }
    } else {
        let id = match id {
            Some(id) => Some(id.to_string()),
            None => sessions::latest(workspace).map(|meta| meta.id),
        };
        if let Some(id) = id {
            tally.absorb(&sessions::load(&id)?.session);
        }
    }

    Ok(tally.finish(fixed))
}

/// The account for one already-loaded session.
///
/// What the desktop app uses: the session it is asking about is the one open in
/// front of it, and re-reading it off disk would be reporting on the transcript
/// as it was last written rather than as it stands.
pub fn of_session(session: &Session, fixed: &Fixed) -> UsageReport {
    let mut tally = Tally::default();
    tally.absorb(session);
    tally.finish(fixed)
}

/// The running counts, before shares and ordering are settled.
#[derive(Default)]
struct Tally {
    sessions: u32,
    turns: u32,
    messages: u32,
    reported_in: u32,
    reported_out: u32,
    cached_in: Option<u32>,
    history: u32,
    by_tool: HashMap<String, ToolUsage>,
    repeats: u32,
    repeat_tokens: u32,
}

impl Tally {
    fn absorb(&mut self, session: &Session) {
        self.sessions += 1;
        self.reported_in += session.usage.input_tokens;
        self.reported_out += session.usage.output_tokens;
        if let Some(cached) = session.usage.cache_read_input_tokens {
            *self.cached_in.get_or_insert(0) += cached;
        }
        self.messages += session.messages.len() as u32;

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
                        entry.name = name.to_string();
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
                        entry.name = name.to_string();
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

    fn finish(self, fixed: &Fixed) -> UsageReport {
        let mut tools: Vec<ToolUsage> = self.by_tool.into_values().collect();
        tools.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.name.cmp(&b.name)));
        let total: u32 = tools.iter().map(|t| t.tokens).sum();
        for tool in &mut tools {
            tool.share = if total == 0 {
                0
            } else {
                (tool.tokens as u64 * 100 / total as u64) as u32
            };
        }

        UsageReport {
            sessions: self.sessions,
            turns: self.turns,
            messages: self.messages,
            reported_in: self.reported_in,
            reported_out: self.reported_out,
            cached_in: self.cached_in,
            history: self.history,
            tools,
            repeats: self.repeats,
            repeat_tokens: self.repeat_tokens,
            system_prompt: fixed.system_prompt,
            schemas: fixed.schemas.clone(),
        }
    }
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

    fn nothing_fixed() -> Fixed {
        Fixed::new("", vec![])
    }

    fn tool<'a>(report: &'a UsageReport, name: &str) -> &'a ToolUsage {
        report
            .tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no {name} in {:?}", report.tools))
    }

    #[test]
    fn a_tools_cost_counts_both_its_arguments_and_its_output() {
        let report = of_session(
            &session_with(vec![
                call("t1", "read_file", "a.rs"),
                result("t1", &"x".repeat(4000)),
            ]),
            &nothing_fixed(),
        );

        let cost = tool(&report, "read_file");
        assert_eq!(cost.calls, 1);
        assert!(cost.tokens >= 1000, "{} tokens", cost.tokens);
    }

    #[test]
    fn an_identical_call_is_counted_as_a_repeat() {
        let report = of_session(
            &session_with(vec![
                call("t1", "grep", "TODO"),
                result("t1", "hit"),
                call("t2", "grep", "TODO"),
                result("t2", "hit"),
            ]),
            &nothing_fixed(),
        );

        assert_eq!(report.repeats, 1);
        assert_eq!(tool(&report, "grep").calls, 2);
    }

    #[test]
    fn a_different_input_is_not_a_repeat() {
        let report = of_session(
            &session_with(vec![
                call("t1", "grep", "TODO"),
                call("t2", "grep", "FIXME"),
            ]),
            &nothing_fixed(),
        );
        assert_eq!(report.repeats, 0);
    }

    #[test]
    fn a_failed_call_is_counted_as_one() {
        let report = of_session(
            &session_with(vec![
                call("t1", "shell", "x"),
                Message::new(Role::User, vec![ContentBlock::tool_error("t1", "boom")]),
            ]),
            &nothing_fixed(),
        );
        assert_eq!(tool(&report, "shell").failures, 1);
    }

    #[test]
    fn typed_messages_are_turns_and_tool_results_are_not() {
        let report = of_session(
            &session_with(vec![
                Message::user("do a thing"),
                call("t1", "shell", "x"),
                result("t1", "ok"),
                Message::assistant("done"),
                Message::user("and another"),
            ]),
            &nothing_fixed(),
        );
        assert_eq!(report.turns, 2);
    }

    #[test]
    fn absorbing_two_sessions_adds_them_up_without_inventing_repeats() {
        let mut tally = Tally::default();
        tally.absorb(&session_with(vec![call("t1", "grep", "a")]));
        tally.absorb(&session_with(vec![call("t1", "grep", "a")]));
        let report = tally.finish(&nothing_fixed());

        assert_eq!(tool(&report, "grep").calls, 2);
        assert_eq!(report.sessions, 2);
        // Two conversations that each asked once. Only a call repeated inside
        // one conversation is work that was already done.
        assert_eq!(report.repeats, 0);
    }

    #[test]
    fn tools_come_back_heaviest_first_with_shares_that_add_up() {
        let report = of_session(
            &session_with(vec![
                call("t1", "grep", "a"),
                result("t1", &"x".repeat(8000)),
                call("t2", "list_dir", "b"),
                result("t2", "one line"),
            ]),
            &nothing_fixed(),
        );

        assert_eq!(report.tools[0].name, "grep");
        // Ordering is the whole reason the app can draw a bar per row without
        // sorting again, so it is settled here rather than by either caller.
        assert!(report.tools[0].tokens > report.tools[1].tokens);
        let total: u32 = report.tools.iter().map(|t| t.share).sum();
        assert!((98..=100).contains(&total), "shares summed to {total}");
    }

    #[test]
    fn a_session_with_no_tool_calls_reports_no_tools_rather_than_failing() {
        let report = of_session(
            &session_with(vec![Message::user("hello"), Message::assistant("hi")]),
            &nothing_fixed(),
        );
        assert!(report.tools.is_empty());
        assert_eq!(report.turns, 1);
        // It read a transcript, so it is not the "nothing recorded" case —
        // which is a different sentence in both frontends.
        assert!(!report.is_empty());
    }

    #[test]
    fn schemas_are_ordered_and_summed_without_the_transcript() {
        // The fixed cost is the half that does not come from a transcript, and
        // an empty workspace still gets it — the number is most useful before
        // running anything, which is exactly when there is no history.
        let fixed = Fixed::new(
            "a system prompt",
            vec![
                ToolDef {
                    name: "small".into(),
                    description: "s".into(),
                    input_schema: json!({}),
                },
                ToolDef {
                    name: "large".into(),
                    description: "d".repeat(400),
                    input_schema: json!({ "properties": { "path": { "type": "string" } } }),
                },
            ],
        );
        let report = Tally::default().finish(&fixed);

        assert!(report.is_empty());
        assert_eq!(report.schemas[0].name, "large");
        assert_eq!(
            report.schema_tokens(),
            report.schemas.iter().map(|s| s.tokens).sum::<u32>()
        );
        assert_eq!(
            report.fixed_tokens(),
            report.system_prompt + report.schema_tokens()
        );
        assert!(report.system_prompt > 0);
    }
}
