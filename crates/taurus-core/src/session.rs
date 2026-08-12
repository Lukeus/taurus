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
    /// - The same tool was called again later with the same input, so the
    ///   earlier answer has been superseded by one already in the transcript.
    /// - It is old enough to be outside the tail kept verbatim, in which case
    ///   the model is working from conclusions rather than from the bytes.
    ///
    /// The block itself always stays: replacing its text keeps every tool call
    /// paired with a result, which is what providers actually validate. Errors
    /// are left alone — they are short, and they are usually the reason the
    /// next few messages look the way they do.
    ///
    /// Costs no model call, which is the point: this runs before summarizing.
    pub fn trim_tool_results(&mut self, keep_recent: usize) -> Trimmed {
        let cutoff = self.messages.len().saturating_sub(keep_recent);
        if cutoff == 0 {
            return Trimmed::default();
        }
        let (call_of, last_use) = index_calls(&self.messages);

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

                let superseded = last_use
                    .get(call.signature.as_str())
                    .is_some_and(|&last| last > index);
                let replacement = if superseded {
                    superseded_note(&call.name)
                } else if content.len() > MIN_SQUEEZE_BYTES {
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

/// Results shorter than this are left alone: the note that would replace one
/// costs most of what shortening it saves.
///
/// It must also exceed head-plus-note, or an already-shortened result would be
/// shortened again on the next pass and a long turn would erode its own history
/// a little at a time. `trimming_twice_changes_nothing_the_second_time` holds
/// that.
const MIN_SQUEEZE_BYTES: usize = 600;

/// How much of a shortened result survives. Enough to keep what the output was
/// *about* — the path, the header line, the first hits — without the body.
const SQUEEZE_HEAD_BYTES: usize = 400;

/// The call a tool result answers.
struct Call {
    name: String,
    /// Tool name plus serialized input. Two calls with the same signature ask
    /// the same question, so the later answer is the one worth keeping.
    signature: String,
}

/// Maps every tool-use id to its call, and every call signature to the last
/// message that made it.
fn index_calls(messages: &[Message]) -> (HashMap<String, Call>, HashMap<String, usize>) {
    let mut call_of = HashMap::new();
    let mut last_use = HashMap::new();
    for (index, message) in messages.iter().enumerate() {
        for (id, name, input) in message.tool_uses() {
            let signature = format!("{name}\u{0}{input}");
            last_use.insert(signature.clone(), index);
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

fn superseded_note(name: &str) -> String {
    format!(
        "[dropped: `{name}` was called again later with the same input, and that result is \
         further down. This one said the same thing.]"
    )
}

fn squeeze(content: &str, name: &str) -> String {
    let head = head_lines(content, SQUEEZE_HEAD_BYTES);
    format!(
        "{head}\n[shortened: {} of {} characters of this older `{name}` result were dropped to \
         fit the context window. Call `{name}` again if you need the rest.]",
        content.len() - head.len(),
        content.len()
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

fn estimate_message(message: &Message) -> u32 {
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
        let trimmed = session.trim_tool_results(2);
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
        session.trim_tool_results(2);
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
        let trimmed = session.trim_tool_results(2);
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
            Message::assistant("a"),
            Message::assistant("b"),
        ];

        assert!(!session.trim_tool_results(2).is_empty());
        let after_one = session.messages.clone();

        assert!(session.trim_tool_results(2).is_empty());
        assert_eq!(session.messages, after_one);
    }

    #[test]
    fn recent_results_are_left_alone() {
        let mut session = Session::new("m");
        session.messages = vec![call("t1", "shell", "x"), result("t1", &bulky())];
        assert!(session.trim_tool_results(8).is_empty());
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
        assert!(session.trim_tool_results(2).is_empty());
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
        assert!(session.trim_tool_results(2).is_empty());
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
        session.trim_tool_results(1);

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
        let trimmed = session.trim_tool_results(2);
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
