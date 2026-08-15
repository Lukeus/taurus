//! Streaming events and the accumulator that turns them back into a `Message`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::message::{ContentBlock, Message, Role};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StopReason {
    /// Model finished its turn and wants the user to speak.
    EndTurn,
    /// Model emitted at least one tool call and is waiting on results.
    ToolUse,
    MaxTokens,
    /// Hit a configured stop sequence.
    StopSequence,
    Canceled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl TokenUsage {
    pub fn total(&self) -> u32 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// The single stream shape every provider adapter must produce.
///
/// Providers that deliver tool calls atomically rather than incrementally
/// (Ollama does this) emit `ToolUseStart`, one `ToolUseInputDelta` carrying the
/// whole JSON, then `ToolUseEnd`. Consumers cannot tell the difference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum StreamEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolUseStart {
        id: String,
        name: String,
    },
    /// A fragment of the tool input JSON. Fragments concatenate in order.
    ToolUseInputDelta {
        id: String,
        json: String,
    },
    ToolUseEnd {
        id: String,
    },
    /// Proof of origin for the thinking block currently open.
    ///
    /// Emitted by adapters whose provider issues one and demands it back
    /// unedited on the next request. Arrives while the block is still open, so
    /// there is no separate close event for reasoning the way there is for a
    /// tool call.
    ThinkingSignature {
        signature: String,
    },
    Usage {
        usage: TokenUsage,
    },
}

/// Rebuilds an assistant `Message` from a `StreamEvent` sequence.
///
/// Tolerant by design: a tool call whose input never parses as JSON becomes a
/// `ToolUse` with a null input, and the tool layer reports the schema error
/// back to the model. Small local models produce malformed tool JSON often
/// enough that killing the turn over it is the wrong behavior.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    blocks: Vec<ContentBlock>,
    /// Index into `blocks` of the block currently being appended to, plus the
    /// raw JSON buffer when that block is a tool use.
    open: Option<Open>,
    usage: TokenUsage,
    malformed_tool_input: Vec<String>,
}

#[derive(Debug)]
enum Open {
    Text(usize),
    Thinking(usize),
    ToolUse { index: usize, json: String },
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::TextDelta { text } => self.append_text(text, false),
            StreamEvent::ThinkingDelta { text } => self.append_text(text, true),
            StreamEvent::ToolUseStart { id, name } => {
                self.close_open();
                self.blocks.push(ContentBlock::ToolUse {
                    id,
                    name,
                    input: serde_json::Value::Null,
                });
                self.open = Some(Open::ToolUse {
                    index: self.blocks.len() - 1,
                    json: String::new(),
                });
            }
            StreamEvent::ToolUseInputDelta { json, .. } => {
                if let Some(Open::ToolUse { json: buf, .. }) = self.open.as_mut() {
                    buf.push_str(&json);
                }
            }
            StreamEvent::ToolUseEnd { .. } => self.close_open(),
            // Attached to the open block rather than closing it: the provider
            // sends this before the block ends, and a signature with no
            // thinking to sign is nothing to keep.
            StreamEvent::ThinkingSignature { signature } => {
                if let Some(Open::Thinking(i)) = self.open {
                    if let Some(ContentBlock::Thinking {
                        signature: slot, ..
                    }) = self.blocks.get_mut(i)
                    {
                        *slot = Some(signature);
                    }
                }
            }
            StreamEvent::Usage { usage } => self.usage = usage,
        }
    }

    fn append_text(&mut self, text: String, thinking: bool) {
        let reuse = match (&self.open, thinking) {
            (Some(Open::Text(i)), false) | (Some(Open::Thinking(i)), true) => Some(*i),
            _ => None,
        };
        match reuse {
            Some(i) => {
                if let Some(
                    ContentBlock::Text { text: buf } | ContentBlock::Thinking { text: buf, .. },
                ) = self.blocks.get_mut(i)
                {
                    buf.push_str(&text);
                }
            }
            None => {
                self.close_open();
                self.blocks.push(if thinking {
                    ContentBlock::Thinking {
                        text,
                        signature: None,
                    }
                } else {
                    ContentBlock::Text { text }
                });
                let i = self.blocks.len() - 1;
                self.open = Some(if thinking {
                    Open::Thinking(i)
                } else {
                    Open::Text(i)
                });
            }
        }
    }

    fn close_open(&mut self) {
        if let Some(Open::ToolUse { index, json }) = self.open.take() {
            let parsed = if json.trim().is_empty() {
                Some(serde_json::json!({}))
            } else {
                serde_json::from_str(&json).ok()
            };
            match (parsed, self.blocks.get_mut(index)) {
                (Some(value), Some(ContentBlock::ToolUse { input, .. })) => *input = value,
                (None, Some(ContentBlock::ToolUse { id, .. })) => {
                    self.malformed_tool_input.push(id.clone());
                }
                _ => {}
            }
        }
    }

    /// Tool-use ids whose input JSON failed to parse. The agent loop turns
    /// these into error tool results so the model can retry.
    pub fn malformed_tool_inputs(&self) -> &[String] {
        &self.malformed_tool_input
    }

    pub fn usage(&self) -> TokenUsage {
        self.usage
    }

    pub fn finish(mut self) -> (Message, TokenUsage, Vec<String>) {
        self.close_open();
        let message = Message::new(Role::Assistant, self.blocks);
        (message, self.usage, self.malformed_tool_input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> StreamEvent {
        StreamEvent::TextDelta { text: s.into() }
    }

    #[test]
    fn merges_consecutive_text_deltas() {
        let mut acc = StreamAccumulator::new();
        acc.push(text("Hel"));
        acc.push(text("lo"));
        let (msg, _, _) = acc.finish();
        assert_eq!(msg.content, vec![ContentBlock::text("Hello")]);
    }

    #[test]
    fn separates_thinking_from_text() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::ThinkingDelta { text: "hmm".into() });
        acc.push(text("answer"));
        let (msg, _, _) = acc.finish();
        assert_eq!(
            msg.content,
            vec![ContentBlock::thinking("hmm"), ContentBlock::text("answer")]
        );
        assert_eq!(msg.text(), "answer");
    }

    #[test]
    fn assembles_tool_use_from_fragments() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "read_file".into(),
        });
        acc.push(StreamEvent::ToolUseInputDelta {
            id: "t1".into(),
            json: "{\"path\":".into(),
        });
        acc.push(StreamEvent::ToolUseInputDelta {
            id: "t1".into(),
            json: "\"a.txt\"}".into(),
        });
        acc.push(StreamEvent::ToolUseEnd { id: "t1".into() });
        let (msg, _, bad) = acc.finish();
        assert!(bad.is_empty());
        assert_eq!(
            msg.content,
            vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }]
        );
    }

    #[test]
    fn reports_malformed_tool_input_without_dropping_the_call() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "read_file".into(),
        });
        acc.push(StreamEvent::ToolUseInputDelta {
            id: "t1".into(),
            json: "{not json".into(),
        });
        acc.push(StreamEvent::ToolUseEnd { id: "t1".into() });
        let (msg, _, bad) = acc.finish();
        assert_eq!(bad, vec!["t1".to_string()]);
        assert!(msg.has_tool_use());
    }

    #[test]
    fn empty_tool_input_becomes_empty_object() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "list_dir".into(),
        });
        acc.push(StreamEvent::ToolUseEnd { id: "t1".into() });
        let (msg, _, bad) = acc.finish();
        assert!(bad.is_empty());
        let (_, _, input) = msg.tool_uses().next().unwrap();
        assert_eq!(input, &serde_json::json!({}));
    }

    #[test]
    fn text_after_tool_use_starts_a_new_block() {
        let mut acc = StreamAccumulator::new();
        acc.push(text("before"));
        acc.push(StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "x".into(),
        });
        acc.push(StreamEvent::ToolUseEnd { id: "t1".into() });
        acc.push(text("after"));
        let (msg, _, _) = acc.finish();
        assert_eq!(msg.content.len(), 3);
        assert_eq!(msg.text(), "beforeafter");
    }
}
