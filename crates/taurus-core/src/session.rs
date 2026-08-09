//! Conversation state and the context-window budget.

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
}

/// ~4 characters per token, the standard approximation for English and code.
fn estimate_message(message: &Message) -> u32 {
    let chars: usize = message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } | ContentBlock::Thinking { text } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.len(),
            ContentBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
            // Images cost far more than their base64 length suggests; a flat
            // estimate is closer than counting characters.
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
