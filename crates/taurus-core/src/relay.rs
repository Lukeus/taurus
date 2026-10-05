//! Handing a long turn to a fresh context instead of summarizing it in place.
//!
//! Compaction keeps a turn alive by replacing the older half of its history
//! with a paragraph and keeping the recent tail verbatim. That tail is the
//! bulkiest and least durable part of the history — tool output the model has
//! already acted on — and compaction only ever shrinks the history to half the
//! budget, so a long turn spends the rest of its life near the ceiling, which
//! is where a small model is worst.
//!
//! A relay leg replaces the *whole* history with a brief, and the brief is
//! mostly not written by the model. The harness already knows exactly what the
//! request was, which files changed, which commands ran and how they ended,
//! and what the last call answered, so it writes those itself. The model is
//! asked only for the one thing the harness can't know: what it learned that
//! isn't in the files yet. If that request fails, the leg still happens, with
//! the parts that can't fail.
//!
//! A leg is a compaction strategy, not a new agent. It is the same session,
//! transcript and rewind: [`crate::Session::summarize_front`] does the
//! swapping, exactly as it does for a summary.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use taurus_provider::{ContentBlock, Message, Role};
use ts_rs::TS;

/// How a turn makes room once trimming old tool output isn't enough.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ContextStrategy {
    /// Summarize the older half of the history, keep the recent tail.
    #[default]
    Compact,
    /// Replace the whole history with a brief, and carry on from it.
    Relay,
}

/// The least number of rounds a leg runs before a symptom may end it early.
///
/// A leg that has just started has nothing to forget yet, and the first
/// rounds after a handover are where a model re-reads the files the brief
/// names, which looks exactly like the repeated reads the symptom counts.
pub(crate) const MIN_EARLY_ROUNDS: u32 = 4;

/// How full the window has to be before a symptom ends a leg early.
///
/// Below this the context is not what's wrong: a model that repeats itself
/// in a half-empty window would repeat itself in a fresh one too, and a
/// handover would cost a request to find that out. See the measurement in
/// `examples/relay.rs`.
pub(crate) const EARLY_SHARE: f32 = 0.5;

/// Commands the brief lists, newest last. Older ones are counted.
const COMMANDS_SHOWN: usize = 12;

/// The most of the request the brief repeats.
const REQUEST_CHARS: usize = 4_000;

/// The most of each of the last call's results the brief repeats.
const LAST_RESULT_CHARS: usize = 1_500;

/// One command the turn ran, as the brief lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ran {
    pub command: String,
    pub outcome: String,
}

/// Where a turn stands with relaying.
///
/// Built for every turn and consulted only under [`ContextStrategy::Relay`],
/// so the control strategy's code path stays the one it always was.
pub(crate) struct Relay {
    /// Handovers so far this turn, so the leg running now is one more.
    pub handovers: u32,
    /// Rounds since the last handover, or since the turn began.
    pub rounds: u32,
    /// Signs since the last handover that the context is working against the
    /// model. See [`Relay::observe_round`].
    pub symptoms: u32,
    /// What the user asked for, when this turn is the one they typed.
    ///
    /// `None` for a continued turn, whose message is the harness's own
    /// "carry on" prompt. The request is then somewhere earlier in the
    /// history, and the model's notes carry its goal instead.
    pub request: Option<String>,
    /// Every command run this turn, across legs. A handover drops the calls
    /// that ran them, so this is the only record the next leg has.
    pub commands: Vec<Ran>,
    /// Read-only calls already answered this leg, as name and input, so an
    /// exact repeat can be told from a new question.
    seen: HashSet<(String, String)>,
}

impl Relay {
    pub fn new(request: Option<String>) -> Self {
        Self {
            handovers: 0,
            rounds: 0,
            symptoms: 0,
            request: request.filter(|r| !r.trim().is_empty()),
            commands: Vec::new(),
            seen: HashSet::new(),
        }
    }

    /// Counts what one finished round says about the context.
    ///
    /// Two symptoms, both already tracked elsewhere for other reasons:
    ///
    /// - a round that failed exactly as an earlier one did (`repeats` from the
    ///   stall detector, counted at two so it fires before the stall limit
    ///   ends the turn);
    /// - a read-only call repeated with exactly the input it had earlier this
    ///   leg, which is the model asking a question it already has the answer
    ///   to somewhere in its context — the same waste the Context panel's
    ///   "Repeated calls" row counts.
    ///
    /// `is_read` decides which calls count as questions. A repeated write is
    /// the model changing something twice, not forgetting.
    pub fn observe_round(
        &mut self,
        assistant: &Message,
        repeats: u32,
        is_read: &dyn Fn(&str) -> bool,
    ) {
        self.rounds += 1;
        if repeats >= 2 {
            self.symptoms += 1;
        }
        for block in &assistant.content {
            let ContentBlock::ToolUse { name, input, .. } = block else {
                continue;
            };
            if !is_read(name) {
                continue;
            }
            if !self.seen.insert((name.clone(), input.to_string())) {
                self.symptoms += 1;
            }
        }
    }

    /// Whether a leg should end before the window is full.
    pub fn due_early(&self, used: u32, budget: u32) -> bool {
        self.rounds >= MIN_EARLY_ROUNDS
            && self.symptoms > 0
            && used as f32 >= budget as f32 * EARLY_SHARE
    }

    /// Resets what is counted per leg, after a handover.
    pub fn start_leg(&mut self) {
        self.handovers += 1;
        self.rounds = 0;
        self.symptoms = 0;
        self.seen.clear();
    }

    /// Adds the commands in `messages` to the turn's record, in order.
    pub fn collect_commands(&mut self, messages: &[Message]) {
        self.commands.extend(commands_in(messages));
    }
}

/// Every `run_command` call in `messages`, with how it ended.
///
/// Read from the history rather than tracked as the calls run, because the
/// history is already the record and a second copy could disagree with it.
fn commands_in(messages: &[Message]) -> Vec<Ran> {
    let mut out = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        for block in &message.content {
            let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            else {
                continue;
            };
            if name != "run_command" {
                continue;
            }
            let Some(command) = input.get("command").and_then(|c| c.as_str()) else {
                continue;
            };
            let background = input.get("background").and_then(|b| b.as_bool()) == Some(true);
            let result = messages
                .get(index + 1)
                .and_then(|next| result_for(next, id));
            let outcome = match result {
                None => "no result".to_string(),
                Some((_, true)) => "refused or failed to start".to_string(),
                Some(_) if background => "started in the background".to_string(),
                Some((text, false)) => outcome_of(&text),
            };
            out.push(Ran {
                command: one_line(command, 200),
                outcome,
            });
        }
    }
    out
}

/// How a finished command ended, read off the first line `run_command`
/// writes. A zero exit writes no line of its own.
fn outcome_of(text: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    if let Some(code) = first.strip_prefix("Exit code ") {
        return format!("exit {}", code.trim());
    }
    if first.starts_with("Killed by signal") {
        return "killed".to_string();
    }
    "exit 0".to_string()
}

/// The result answering call `id` in `message`, as text and whether it was an
/// error.
fn result_for(message: &Message, id: &str) -> Option<(String, bool)> {
    message.content.iter().find_map(|block| match block {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } if tool_use_id == id => Some((content.to_text().into_owned(), *is_error)),
        _ => None,
    })
}

/// What the model wrote for the next leg.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Notes {
    pub goal: String,
    pub learned: Vec<String>,
    pub next: String,
}

/// The shape the notes are asked for in.
///
/// Three fields, all required, and small for the same reason the compaction
/// summary's schema is: a constraint on sampling spends the model's capacity
/// on bookkeeping. `next` is the field that matters most. A leg that starts
/// without knowing its next step spends its first rounds working one out
/// from the files, which is the re-reading the handover was meant to save.
pub(crate) fn notes_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "goal": { "type": "string" },
            "learned": { "type": "array", "items": { "type": "string" } },
            "next": { "type": "string" },
        },
        "required": ["goal", "learned", "next"],
    })
}

/// What the model is told when it writes its notes.
pub(crate) const NOTES_SYSTEM: &str = "\
You're about to hand this task to a fresh copy of yourself whose context will \
hold only a short brief. The harness already puts the request, the files \
changed, the commands run and their exit codes, and your last tool call in \
that brief, so don't repeat them. Answer as JSON matching the schema. `goal` \
is what the user wants, in one sentence. `learned` is what you found out that \
isn't written in any file yet: causes you pinned down, approaches that failed \
and why, names and locations you'll need again. One line each, at most ten. \
`next` is the very next thing to do, specifically enough to start on without \
looking anything up.";

/// The notes, out of whatever the model answered.
///
/// A backend that can't enforce a schema answers in prose, and that prose is
/// still the model's notes, so it is kept whole as a single learned line
/// rather than thrown away. An answer that parses but has nothing in it is no
/// notes at all.
pub(crate) fn parse_notes(text: &str) -> Option<Notes> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str::<serde_json::Value>(text)
    else {
        return Some(Notes {
            learned: vec![text.to_string()],
            ..Notes::default()
        });
    };
    let string = |key: &str| {
        object
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let learned: Vec<String> = object
        .get("learned")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let notes = Notes {
        goal: string("goal"),
        learned,
        next: string("next"),
    };
    if notes.goal.is_empty() && notes.learned.is_empty() && notes.next.is_empty() {
        return None;
    }
    Some(notes)
}

/// Everything the brief is made from.
pub(crate) struct Baton<'a> {
    /// The leg this brief starts, counting the turn's first as 1.
    pub leg: u32,
    pub request: Option<&'a str>,
    pub notes: Result<Notes, String>,
    pub files: &'a [String],
    pub commands: &'a [Ran],
    /// The history as it stood just before the handover, for the last
    /// exchange.
    pub messages: &'a [Message],
}

/// The message the next leg starts from.
pub(crate) fn brief(baton: &Baton<'_>) -> String {
    let mut out = format!(
        "You're picking up a task part way through: this is leg {} of it. Your earlier \
         working context was cleared to make room, and this brief replaces it. The harness \
         wrote everything here except \"Your notes\", which you wrote just before the \
         handover, so the files and commands below are exact.",
        baton.leg
    );

    out.push_str("\n\n## The request\n\n");
    match baton.request {
        Some(request) => out.push_str(&clip(request.trim(), REQUEST_CHARS)),
        None => out.push_str(
            "This turn continues an earlier one, so the request isn't repeated here. The goal \
             in your notes is what it asked for.",
        ),
    }

    out.push_str("\n\n## Your notes\n\n");
    match &baton.notes {
        Ok(notes) => {
            if !notes.goal.is_empty() {
                out.push_str(&format!("Goal: {}\n", notes.goal));
            }
            for line in &notes.learned {
                out.push_str(&format!("- {line}\n"));
            }
            if notes.next.is_empty() {
                out.push_str("Next: not recorded. Work it out from the request and the files.");
            } else {
                out.push_str(&format!("Next: {}", notes.next));
            }
        }
        Err(reason) => out.push_str(&format!(
            "None were written ({reason}). Work from the rest of this brief and the files."
        )),
    }

    out.push_str("\n\n## Files changed so far this turn\n\n");
    if baton.files.is_empty() {
        out.push_str("None yet.");
    } else {
        let list: Vec<String> = baton.files.iter().map(|f| format!("- {f}")).collect();
        out.push_str(&list.join("\n"));
    }

    out.push_str("\n\n## Commands run so far this turn\n\n");
    if baton.commands.is_empty() {
        out.push_str("None yet.");
    } else {
        let skipped = baton.commands.len().saturating_sub(COMMANDS_SHOWN);
        if skipped > 0 {
            out.push_str(&format!("({skipped} earlier ones not shown.)\n"));
        }
        let list: Vec<String> = baton.commands[skipped..]
            .iter()
            .map(|ran| format!("- `{}` → {}", ran.command, ran.outcome))
            .collect();
        out.push_str(&list.join("\n"));
    }

    if let Some(last) = last_exchange(baton.messages) {
        out.push_str("\n\n## Where you left off\n\n");
        out.push_str(&last);
    }

    out.push_str(
        "\n\nRead a file again before you edit it: what you knew of its contents went with the \
         old context. If you have a plan, it's restated at the end of every request, as always.",
    );
    out
}

/// The last thing the model did and what came back, rendered as text.
///
/// The model's own last words and calls, then the answers to them, or the
/// last thing the harness said to it — a nudge, a background report — when
/// that is what the history ends on. Text rather than the original blocks,
/// because a tool result without its call in front of it is a request every
/// provider refuses.
fn last_exchange(messages: &[Message]) -> Option<String> {
    let at = messages.iter().rposition(|m| m.role == Role::Assistant)?;
    let assistant = &messages[at];
    let mut out = String::new();

    let said = assistant.text();
    if !said.trim().is_empty() {
        out.push_str(&format!("You said: {}\n", clip(said.trim(), 1_000)));
    }
    let calls: Vec<(&str, &str, &serde_json::Value)> = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
        .collect();
    let after = messages.get(at + 1);
    for (id, name, input) in calls {
        out.push_str(&format!(
            "You called `{name}` with {}",
            one_line(&input.to_string(), 300)
        ));
        match after.and_then(|m| result_for(m, id)) {
            Some((text, is_error)) => {
                let label = if is_error { "It failed" } else { "It answered" };
                out.push_str(&format!(
                    ". {label}:\n\n```\n{}\n```\n",
                    clip(text.trim_end(), LAST_RESULT_CHARS)
                ));
            }
            None => out.push_str(". It has no answer on record.\n"),
        }
    }
    // Text after the results, or a message of its own: what the harness said
    // last, which the model hasn't answered yet.
    for message in &messages[at + 1..] {
        let text: Vec<&str> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if !text.is_empty() {
            out.push_str(&format!(
                "\nThen you were told:\n\n{}\n",
                clip(text.join("\n\n").trim(), LAST_RESULT_CHARS)
            ));
        }
    }

    let out = out.trim_end().to_string();
    (!out.is_empty()).then_some(out)
}

/// `text` on one line, cut to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    clip(&flat, max)
}

/// `text` cut to `max` characters, saying so when it was.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head} … (cut)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str, input: serde_json::Value) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(id, name, input)],
        )
    }

    fn answer(id: &str, text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::tool_result(id, text)])
    }

    #[test]
    fn commands_are_read_with_how_they_ended() {
        let messages = vec![
            call(
                "a",
                "run_command",
                serde_json::json!({ "command": "cargo test" }),
            ),
            answer("a", "Exit code 101\nfailures:"),
            call(
                "b",
                "run_command",
                serde_json::json!({ "command": "cargo build" }),
            ),
            answer("b", "Finished"),
            call(
                "c",
                "run_command",
                serde_json::json!({ "command": "pnpm dev", "background": true }),
            ),
            answer("c", "Started job 1"),
            call("d", "read_file", serde_json::json!({ "path": "a.rs" })),
            answer("d", "fn main() {}"),
        ];
        let ran = commands_in(&messages);
        let outcomes: Vec<(&str, &str)> = ran
            .iter()
            .map(|r| (r.command.as_str(), r.outcome.as_str()))
            .collect();
        assert_eq!(
            outcomes,
            [
                ("cargo test", "exit 101"),
                ("cargo build", "exit 0"),
                ("pnpm dev", "started in the background"),
            ]
        );
    }

    #[test]
    fn a_refused_command_is_not_reported_as_exit_zero() {
        let messages = vec![
            call(
                "a",
                "run_command",
                serde_json::json!({ "command": "rm -rf /" }),
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::tool_error("a", "denied by the user")],
            ),
        ];
        assert_eq!(
            commands_in(&messages)[0].outcome,
            "refused or failed to start"
        );
    }

    #[test]
    fn a_repeated_read_is_a_symptom_and_a_repeated_write_is_not() {
        let is_read = |name: &str| name == "read_file";
        let mut relay = Relay::new(Some("fix it".into()));
        let read = call("a", "read_file", serde_json::json!({ "path": "a.rs" }));
        let write = call("b", "write_file", serde_json::json!({ "path": "a.rs" }));
        relay.observe_round(&read, 0, &is_read);
        relay.observe_round(&write, 0, &is_read);
        relay.observe_round(&write, 0, &is_read);
        assert_eq!(relay.symptoms, 0);
        relay.observe_round(&read, 0, &is_read);
        assert_eq!(relay.symptoms, 1);
        // A handover forgets what was asked, so the first read after it is
        // a new question, not a repeat.
        relay.start_leg();
        relay.observe_round(&read, 0, &is_read);
        assert_eq!(relay.symptoms, 0);
    }

    #[test]
    fn a_failure_seen_twice_is_a_symptom() {
        let mut relay = Relay::new(None);
        let message = Message::new(Role::Assistant, vec![ContentBlock::text("hm")]);
        relay.observe_round(&message, 1, &|_| false);
        assert_eq!(relay.symptoms, 0);
        relay.observe_round(&message, 2, &|_| false);
        assert_eq!(relay.symptoms, 1);
    }

    #[test]
    fn an_early_handover_needs_rounds_a_symptom_and_a_half_full_window() {
        let mut relay = Relay::new(None);
        relay.symptoms = 1;
        relay.rounds = MIN_EARLY_ROUNDS - 1;
        assert!(
            !relay.due_early(9_000, 10_000),
            "too soon after the last one"
        );
        relay.rounds = MIN_EARLY_ROUNDS;
        assert!(relay.due_early(5_000, 10_000));
        assert!(
            !relay.due_early(4_999, 10_000),
            "the window isn't the problem"
        );
        relay.symptoms = 0;
        assert!(!relay.due_early(9_000, 10_000), "nothing has gone wrong");
    }

    #[test]
    fn notes_in_prose_are_kept_and_empty_notes_are_none() {
        assert_eq!(
            parse_notes("The bug is in parse()."),
            Some(Notes {
                learned: vec!["The bug is in parse().".into()],
                ..Notes::default()
            })
        );
        assert_eq!(parse_notes("  "), None);
        assert_eq!(parse_notes(r#"{"goal":"","learned":[],"next":""}"#), None);
        let notes =
            parse_notes(r#"{"goal":"Fix it","learned":[" a ",""],"next":"Run tests"}"#).unwrap();
        assert_eq!(notes.learned, ["a"]);
        assert_eq!(notes.next, "Run tests");
    }

    #[test]
    fn the_brief_carries_every_part_and_says_when_notes_are_missing() {
        let messages = vec![
            call(
                "a",
                "run_command",
                serde_json::json!({ "command": "pytest" }),
            ),
            answer("a", "Exit code 1\nE   assert 2 == 3"),
        ];
        let commands = commands_in(&messages);
        let files = vec!["src/calc.py".to_string()];
        let text = brief(&Baton {
            leg: 2,
            request: Some("Make the tests pass."),
            notes: Err("the model returned nothing".into()),
            files: &files,
            commands: &commands,
            messages: &messages,
        });
        assert!(text.contains("leg 2"), "{text}");
        assert!(text.contains("Make the tests pass."), "{text}");
        assert!(
            text.contains("None were written (the model returned nothing)"),
            "{text}"
        );
        assert!(text.contains("- src/calc.py"), "{text}");
        assert!(text.contains("- `pytest` → exit 1"), "{text}");
        assert!(text.contains("You called `run_command`"), "{text}");
        assert!(text.contains("assert 2 == 3"), "{text}");
    }

    #[test]
    fn the_last_exchange_includes_what_the_harness_said_after_it() {
        let messages = vec![
            Message::new(Role::Assistant, vec![ContentBlock::text("All done.")]),
            Message::user("You changed files and have not run anything since."),
        ];
        let last = last_exchange(&messages).unwrap();
        assert!(last.contains("You said: All done."), "{last}");
        assert!(last.contains("Then you were told:"), "{last}");
        assert!(last.contains("have not run anything"), "{last}");
    }

    #[test]
    fn only_the_newest_commands_are_listed() {
        let commands: Vec<Ran> = (0..20)
            .map(|i| Ran {
                command: format!("step {i}"),
                outcome: "exit 0".into(),
            })
            .collect();
        let text = brief(&Baton {
            leg: 3,
            request: None,
            notes: Ok(Notes {
                next: "go".into(),
                ..Notes::default()
            }),
            files: &[],
            commands: &commands,
            messages: &[],
        });
        assert!(text.contains("(8 earlier ones not shown.)"), "{text}");
        assert!(!text.contains("`step 7`"), "{text}");
        assert!(text.contains("`step 19`"), "{text}");
        assert!(text.contains("continues an earlier one"), "{text}");
    }
}
