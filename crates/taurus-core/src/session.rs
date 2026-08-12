//! Conversation state and the context-window budget.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use taurus_provider::{ContentBlock, Message, Role, TokenUsage};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub model: String,
    pub messages: Vec<Message>,
    /// Cumulative across the session, for the UI's token counter.
    pub usage: TokenUsage,
}

impl Session {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            model: model.into(),
            messages: Vec::new(),
            usage: TokenUsage::default(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn add_usage(&mut self, usage: TokenUsage) {
        self.usage.input_tokens = self.usage.input_tokens.saturating_add(usage.input_tokens);
        self.usage.output_tokens = self.usage.output_tokens.saturating_add(usage.output_tokens);
    }

    /// Rough token count for the whole history.
    ///
    /// Deliberately an estimate: asking the provider to count costs a round
    /// trip per iteration, and the only decision this feeds is "compact now or
    /// later", which tolerates being wrong by a few percent.
    pub fn estimated_tokens(&self) -> u32 {
        self.messages.iter().map(estimate_message).sum()
    }

    /// Shrinks tool results that history no longer needs verbatim.
    ///
    /// Tool output is most of what a working session holds — a file read, a
    /// grep, a build log — and every byte of it is re-sent on every later
    /// iteration of the turn. Two things make a result safe to shorten:
    ///
    /// - The same question was asked again later and answered, so the earlier
    ///   answer has been superseded by one already in the transcript.
    /// - It is old enough to be outside the tail kept verbatim, in which case
    ///   the model is working from conclusions rather than from the bytes.
    ///
    /// Superseding is the strong claim of the two — it throws the output away
    /// rather than shortening it — so it is fenced in three ways, all of which
    /// have to hold. `repeats_supersede` decides whether asking a given tool
    /// twice means anything: a build log or an MCP call answers a moment, not a
    /// question, and two runs of `cargo test` around a fix are the two halves
    /// of the loop the model is in. No tool that changes the world may have
    /// run in between, or the later read answers a different question than the
    /// earlier one — read, edit, read is the shape that makes this matter. And
    /// the later call must have actually produced a result: a turn canceled
    /// between the model's call and the tool's answer leaves a dangling
    /// `tool_use` in history, and pointing at it would delete the only real
    /// output in favor of a note about a result that is not there.
    ///
    /// The block itself always stays: replacing its text keeps every tool call
    /// paired with a result, which is what providers actually validate. Errors
    /// are left alone — they are short, and they are usually the reason the
    /// next few messages look the way they do.
    ///
    /// Costs no model call, which is the point: this runs before summarizing.
    pub fn trim_tool_results(
        &mut self,
        keep_recent: usize,
        repeats_supersede: &dyn Fn(&str) -> bool,
    ) -> Trimmed {
        let cutoff = self.messages.len().saturating_sub(keep_recent);
        if cutoff == 0 {
            return Trimmed::default();
        }
        let (call_of, last_use) = index_calls(&self.messages);
        let mutations = mutations_before(&self.messages, repeats_supersede);

        let mut trimmed = Trimmed::default();
        for (index, message) in self.messages.iter_mut().enumerate().take(cutoff) {
            for block in &mut message.content {
                let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = block
                else {
                    continue;
                };
                if *is_error {
                    continue;
                }
                let Some(call) = call_of.get(tool_use_id.as_str()) else {
                    continue;
                };

                let superseded = repeats_supersede(&call.name)
                    && last_use.get(call.signature.as_str()).is_some_and(|&last| {
                        last > index && mutations[last + 1] == mutations[index]
                    });
                let replacement = if superseded {
                    superseded_note(&call.name)
                } else if content.len() > squeeze_floor(&call.name) {
                    squeeze(content, &call.name)
                } else {
                    continue;
                };

                // A note longer than what it replaces is not a saving.
                if replacement.len() >= content.len() {
                    continue;
                }
                trimmed.results += 1;
                trimmed.tokens_saved += ((content.len() - replacement.len()) / 4) as u32;
                *content = replacement;
            }
        }
        trimmed
    }
}

/// What one [`Session::trim_tool_results`] pass gave back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trimmed {
    /// Tool results collapsed or shortened.
    pub results: usize,
    /// Estimated tokens recovered, on the same ~4-characters-per-token basis
    /// the compaction trigger uses.
    pub tokens_saved: u32,
}

impl Trimmed {
    pub fn is_empty(&self) -> bool {
        self.results == 0
    }
}

/// How much of a shortened result survives. Enough to keep what the output was
/// *about* — the path, the header line, the first hits — without the body.
const SQUEEZE_HEAD_BYTES: usize = 400;

/// Results at or under this are left alone: the note that would replace one
/// costs most of what shortening it saves.
///
/// It is head-plus-note rather than a flat number because the note names the
/// tool twice, and `mcp__server__tool` names run long enough to push an
/// already-shortened result back over any fixed threshold — which would shorten
/// it again on the next pass, and let a long turn erode its own history a
/// little at a time while reporting the loss with a wrong character count.
/// Measuring the note instead of counting its characters by hand keeps that
/// true if the wording changes.
/// `trimming_twice_changes_nothing_the_second_time` holds it.
fn squeeze_floor(name: &str) -> usize {
    SQUEEZE_HEAD_BYTES + squeeze_note(name, usize::MAX, usize::MAX).len()
}

/// The call a tool result answers.
struct Call {
    name: String,
    /// Tool name plus serialized input. Two calls with the same signature ask
    /// the same question, so the later answer is the one worth keeping.
    signature: String,
}

/// Maps every tool-use id to its call, and every call signature to the last
/// message that made it *and got an answer*.
///
/// Answered is the load-bearing part: a call with no result behind it cannot
/// supersede anything, because there is nothing to point the model at. That
/// covers the canceled turn, whose assistant message is in history while the
/// tool's answer never arrived, and the call that failed, where the earlier
/// result is the only one that ever worked.
fn index_calls(messages: &[Message]) -> (HashMap<String, Call>, HashMap<String, usize>) {
    let answered: std::collections::HashSet<&str> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error: false,
                ..
            } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();

    let mut call_of = HashMap::new();
    let mut last_use = HashMap::new();
    for (index, message) in messages.iter().enumerate() {
        for (id, name, input) in message.tool_uses() {
            let signature = format!("{name}\u{0}{input}");
            if answered.contains(id) {
                last_use.insert(signature.clone(), index);
            }
            call_of.insert(
                id.to_string(),
                Call {
                    name: name.to_string(),
                    signature,
                },
            );
        }
    }
    (call_of, last_use)
}

/// A running count of world-changing tool calls, indexed so that
/// `counts[b] - counts[a]` is how many ran in messages `a..b`.
///
/// Two identical reads only ask the same question if nothing moved underneath
/// them. Anything whose repeat does not supersede — a write, a command, an MCP
/// call — is treated as having moved something, which is the conservative
/// reading and the correct one for `edit_file`.
fn mutations_before(messages: &[Message], repeats_supersede: &dyn Fn(&str) -> bool) -> Vec<usize> {
    let mut counts = Vec::with_capacity(messages.len() + 1);
    let mut running = 0;
    counts.push(0);
    for message in messages {
        running += message
            .tool_uses()
            .filter(|(_, name, _)| !repeats_supersede(name))
            .count();
        counts.push(running);
    }
    counts
}

fn superseded_note(name: &str) -> String {
    format!(
        "[dropped: `{name}` was called again later with the same input and nothing changed the \
         answer in between, so that result — further down — is this one.]"
    )
}

fn squeeze(content: &str, name: &str) -> String {
    let head = head_lines(content, SQUEEZE_HEAD_BYTES);
    let note = squeeze_note(name, content.len() - head.len(), content.len());
    format!("{head}\n{note}")
}

fn squeeze_note(name: &str, dropped: usize, total: usize) -> String {
    format!(
        "[shortened: {dropped} of {total} characters of this older `{name}` result were dropped \
         to fit the context window. Call `{name}` again if you need the rest.]"
    )
}

/// A prefix of `text` no longer than `max` bytes, cut at a line break where
/// there is one to cut at and a character boundary otherwise.
fn head_lines(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    // Cutting mid-line is legible but cutting at one is more so, as long as it
    // does not cost most of the excerpt.
    match text[..end].rfind('\n') {
        Some(newline) if newline > max / 2 => &text[..newline],
        _ => &text[..end],
    }
}

/// ~4 characters per token, the standard approximation for English and code.
///
/// Public so that anything reporting on a transcript — `taurus usage`, the
/// token counter — arrives at the same numbers the compaction trigger does. Two
/// estimators would disagree, and the one the user sees would be the wrong one.
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4) as u32
}

/// What a single block costs, on the same basis.
pub fn estimate_block(block: &ContentBlock) -> u32 {
    match block {
        ContentBlock::Text { text } | ContentBlock::Thinking { text } => estimate_tokens(text),
        ContentBlock::ToolResult { content, .. } => estimate_tokens(content),
        ContentBlock::ToolUse { name, input, .. } => {
            estimate_tokens(name) + estimate_tokens(&input.to_string())
        }
        // Images cost far more than their base64 length suggests; a flat
        // estimate is closer than counting characters.
        ContentBlock::Image { .. } => 1000,
    }
}

/// What a whole message costs, blocks plus the envelope around them.
///
/// Public for the same reason [`estimate_tokens`] is: `taurus usage` reports on
/// a transcript the compaction trigger is also measuring, and summing blocks
/// without the envelope would have the two disagree by four tokens a message —
/// small, and exactly the kind of small that makes a user distrust the number.
pub fn estimate_message(message: &Message) -> u32 {
    let chars: usize = message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } | ContentBlock::Thinking { text } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.len(),
            ContentBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
            ContentBlock::Image { .. } => 4000,
        })
        .sum();
    // Per-message envelope overhead (role tokens, delimiters).
    (chars / 4) as u32 + 4
}

/// Splits history into the part to summarize and the part to keep verbatim.
///
/// The tail is kept whole, and never split between an assistant's tool call
/// and the result that answers it: a dangling tool result confuses every
/// provider and a dangling tool call makes some of them error outright.
pub fn split_for_compaction(messages: &[Message], keep_recent: usize) -> (usize, usize) {
    if messages.len() <= keep_recent {
        return (0, messages.len());
    }
    let mut boundary = messages.len() - keep_recent;

    // Walk forward off any tool result whose call would be left behind.
    while boundary < messages.len() && starts_with_tool_result(&messages[boundary]) {
        boundary += 1;
    }
    (boundary, messages.len() - boundary)
}

fn starts_with_tool_result(message: &Message) -> bool {
    message.role == Role::User
        && message
            .content
            .first()
            .is_some_and(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call() -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: serde_json::json!({}),
            }],
        )
    }

    fn tool_result() -> Message {
        Message::new(Role::User, vec![ContentBlock::tool_result("t1", "body")])
    }

    fn call(id: &str, name: &str, path: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input: serde_json::json!({ "path": path }),
            }],
        )
    }

    fn result(id: &str, body: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::tool_result(id, body)])
    }

    /// Stands in for the registry: the read-only tools whose repeat asks the
    /// same question again. Everything else — `shell`, `edit_file`, an MCP
    /// call — answers a moment rather than a question.
    fn reads(name: &str) -> bool {
        matches!(name, "read_file" | "grep" | "list_dir")
    }

    fn body_of(message: &Message) -> &str {
        match &message.content[0] {
            ContentBlock::ToolResult { content, .. } => content,
            other => panic!("expected a tool result, got {other:?}"),
        }
    }

    /// Long enough to be worth shortening.
    fn bulky() -> String {
        "some output line\n".repeat(200)
    }

    #[test]
    fn a_repeated_call_drops_the_earlier_answer() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            result("t2", &bulky()),
            Message::assistant("done"),
        ];
        let trimmed = session.trim_tool_results(2, &reads);
        assert!(!trimmed.is_empty());
        assert!(body_of(&session.messages[1]).contains("called again later"));
        // The answer that is still current survives untouched.
        assert_eq!(body_of(&session.messages[3]), bulky());
    }

    #[test]
    fn a_different_input_is_not_a_repeat() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", "short"),
            call("t2", "read_file", "b.rs"),
            result("t2", "short"),
            Message::assistant("done"),
        ];
        session.trim_tool_results(2, &reads);
        assert_eq!(body_of(&session.messages[1]), "short");
    }

    #[test]
    fn an_old_bulky_result_keeps_its_head_and_says_what_went() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "x"),
            result("t1", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        let trimmed = session.trim_tool_results(2, &reads);
        assert_eq!(trimmed.results, 1);
        assert!(trimmed.tokens_saved > 0);

        let body = body_of(&session.messages[1]);
        assert!(body.starts_with("some output line"), "{body}");
        assert!(body.contains("shortened"), "{body}");
        assert!(body.len() < bulky().len() / 2, "{body}");
    }

    /// The trigger fires on every iteration once history is large, so this runs
    /// repeatedly over the same messages. A second pass that shortened them
    /// again would erode the conversation a little at a time until nothing of
    /// it was left.
    ///
    /// The long name is the case that matters: the note names its tool twice,
    /// so a fixed threshold that clears head-plus-note for `shell` does not
    /// clear it for anything namespaced, and those results would shrink a
    /// little on every pass while reporting a character count that was already
    /// wrong.
    #[test]
    fn trimming_twice_changes_nothing_the_second_time() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            result("t2", &bulky()),
            call("t3", "shell", "cargo test"),
            result("t3", &bulky()),
            call(
                "t4",
                "mcp__documentation__search_reference_articles",
                "a.rs",
            ),
            result("t4", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];

        assert!(!session.trim_tool_results(2, &reads).is_empty());
        let after_one = session.messages.clone();

        assert!(session.trim_tool_results(2, &reads).is_empty());
        assert_eq!(session.messages, after_one);
    }

    /// The read → edit → read loop. The two reads have the same signature and
    /// different answers, and the first one is the only record of what the file
    /// held before the edit.
    #[test]
    fn a_repeat_across_an_edit_does_not_supersede() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "edit_file", "a.rs"),
            result("t2", "edited"),
            call("t3", "read_file", "a.rs"),
            result("t3", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        // Shortened for age is still fine — the head of the pre-edit file
        // survives, which is what the note about it promises.
        assert!(body.starts_with("some output line"), "{body}");
    }

    /// Run a suite, fix, run it again: the first result is the failure the fix
    /// was for, and "this one said the same thing" is the one thing it did not.
    #[test]
    fn a_repeated_command_does_not_supersede() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "cargo test"),
            result("t1", &bulky()),
            call("t2", "shell", "cargo test"),
            result("t2", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        assert!(body.starts_with("some output line"), "{body}");
    }

    /// A turn canceled between the model's call and the tool's answer leaves
    /// the call in history with nothing behind it. Superseding to it would
    /// delete the only real output and point at a result that is not there.
    #[test]
    fn an_unanswered_repeat_supersedes_nothing() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        assert!(body.starts_with("some output line"), "{body}");
    }

    /// A repeat that failed leaves the earlier result the only one that ever
    /// worked.
    #[test]
    fn a_failed_repeat_supersedes_nothing() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            Message::new(Role::User, vec![ContentBlock::tool_error("t2", "gone")]),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        session.trim_tool_results(2, &reads);

        let body = body_of(&session.messages[1]);
        assert!(!body.contains("dropped:"), "{body}");
        assert!(body.starts_with("some output line"), "{body}");
    }

    #[test]
    fn recent_results_are_left_alone() {
        let mut session = Session::new("m");
        session.messages = vec![call("t1", "shell", "x"), result("t1", &bulky())];
        assert!(session.trim_tool_results(8, &reads).is_empty());
        assert_eq!(body_of(&session.messages[1]), bulky());
    }

    #[test]
    fn a_short_result_is_not_worth_shortening() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "x"),
            result("t1", "ok"),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        assert!(session.trim_tool_results(2, &reads).is_empty());
        assert_eq!(body_of(&session.messages[1]), "ok");
    }

    #[test]
    fn failures_survive_trimming() {
        let mut session = Session::new("m");
        let message = bulky();
        session.messages = vec![
            call("t1", "shell", "x"),
            Message::new(Role::User, vec![ContentBlock::tool_error("t1", &message)]),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        assert!(session.trim_tool_results(2, &reads).is_empty());
        assert_eq!(body_of(&session.messages[1]), message);
    }

    #[test]
    fn trimming_leaves_every_call_paired_with_a_result() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "read_file", "a.rs"),
            result("t1", &bulky()),
            call("t2", "read_file", "a.rs"),
            result("t2", &bulky()),
            Message::assistant("done"),
        ];
        let before = session.messages.len();
        session.trim_tool_results(1, &reads);

        assert_eq!(session.messages.len(), before);
        for id in ["t1", "t2"] {
            assert!(
                session.messages.iter().any(|m| m.content.iter().any(|b| {
                    matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id)
                })),
                "the result for {id} must still be there"
            );
        }
    }

    #[test]
    fn trimming_lowers_the_estimate_it_reports_lowering() {
        let mut session = Session::new("m");
        session.messages = vec![
            call("t1", "shell", "x"),
            result("t1", &bulky()),
            Message::assistant("a"),
            Message::assistant("b"),
        ];
        let before = session.estimated_tokens();
        let trimmed = session.trim_tool_results(2, &reads);
        let after = session.estimated_tokens();

        assert!(after < before);
        // Both round chars down to tokens, just at different points, so they
        // agree to within the rounding rather than exactly.
        assert!(
            (before - after).abs_diff(trimmed.tokens_saved) <= 1,
            "reported {} saved, estimate fell by {}",
            trimmed.tokens_saved,
            before - after
        );
    }

    #[test]
    fn estimate_grows_with_content() {
        let mut session = Session::new("m");
        let small = session.estimated_tokens();
        session.push(Message::user("x".repeat(4000)));
        assert!(session.estimated_tokens() > small + 900);
    }

    #[test]
    fn usage_accumulates_across_turns() {
        let mut session = Session::new("m");
        session.add_usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
        });
        session.add_usage(TokenUsage {
            input_tokens: 3,
            output_tokens: 2,
        });
        assert_eq!(session.usage.total(), 20);
    }

    #[test]
    fn nothing_is_dropped_when_history_is_short() {
        let messages = vec![Message::user("a"), Message::assistant("b")];
        assert_eq!(split_for_compaction(&messages, 6), (0, 2));
    }

    #[test]
    fn compaction_never_orphans_a_tool_result() {
        // Boundary would land on the tool result, separating it from the call.
        let messages = vec![
            Message::user("a"),
            tool_call(),
            tool_result(),
            Message::assistant("done"),
        ];
        let (dropped, kept) = split_for_compaction(&messages, 2);
        assert_eq!(dropped + kept, messages.len());
        assert!(
            !starts_with_tool_result(&messages[dropped]),
            "kept history must not begin with an orphan tool result"
        );
    }

    #[test]
    fn consecutive_tool_results_are_all_skipped() {
        let messages = vec![
            Message::user("a"),
            tool_call(),
            tool_result(),
            tool_result(),
            Message::assistant("done"),
        ];
        let (dropped, _) = split_for_compaction(&messages, 3);
        assert!(!starts_with_tool_result(&messages[dropped]));
    }
}
