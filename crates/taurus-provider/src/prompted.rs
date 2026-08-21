//! Tool calling for models with no native tool support.
//!
//! Several widely used local models (`gemma3`, most base models) accept no
//! `tools` parameter at all. Rather than let the harness degrade to chat-only
//! on those models, this module teaches them tool calling through the system
//! prompt and parses the result back into the exact same [`StreamEvent`]
//! sequence a native adapter produces. `taurus-core` cannot tell which path a
//! turn took.
//!
//! The wire format is the `<tool_call>` convention used by Qwen and Hermes
//! fine-tunes, chosen over a ```json fence because prose and code answers
//! contain JSON fences constantly and would false-positive.

use crate::message::{ContentBlock, Message};
use crate::request::{ChatRequest, ToolDef};
use crate::stream::StreamEvent;

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";

/// Name reported when a `<tool_call>` block contains unparseable JSON.
///
/// Emitting a call with this name (rather than swallowing the block) keeps the
/// loop closed: the tool layer returns a "no such tool" error result and the
/// model gets a chance to fix its syntax on the next iteration. Silently
/// dropping the block would leave the model waiting for a result that never
/// arrives.
pub const MALFORMED_TOOL: &str = "__malformed_tool_call__";

pub struct PromptedTools;

impl PromptedTools {
    /// Rewrites a request so a tool-less model can still call tools: tool
    /// definitions move into the system prompt, and tool-use / tool-result
    /// blocks in the history are re-rendered as plain text in the same format
    /// the model is being asked to produce.
    pub fn rewrite(request: &mut ChatRequest) {
        if request.tools.is_empty() {
            return;
        }
        let instructions = Self::system_suffix(&request.tools);
        request.system = Some(match request.system.take() {
            Some(existing) if !existing.trim().is_empty() => {
                format!("{existing}\n\n{instructions}")
            }
            _ => instructions,
        });
        request.messages = request.messages.iter().map(Self::flatten_message).collect();
        request.tools.clear();
    }

    pub fn system_suffix(tools: &[ToolDef]) -> String {
        let mut s =
            String::from("# Tool use\n\nYou can call tools. These are available:\n\n<tools>\n");
        for tool in tools {
            let line = serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            });
            s.push_str(&line.to_string());
            s.push('\n');
        }
        s.push_str(
            "</tools>\n\n\
             To call a tool, emit a block in exactly this form:\n\n\
             <tool_call>\n\
             {\"name\": \"tool_name\", \"input\": {\"arg\": \"value\"}}\n\
             </tool_call>\n\n\
             Rules:\n\
             - The block contains one JSON object and nothing else. No commentary inside it.\n\
             - `input` must satisfy that tool's input_schema.\n\
             - Stop generating after the closing tag and wait for the result. \
             Never write the result yourself.\n\
             - To call several tools, emit several blocks in a row.\n\
             - When you need no tool, answer normally and emit no block.\n",
        );
        s
    }

    /// Renders tool-use and tool-result blocks as text, leaving everything else
    /// untouched. A model that never saw a `tools` parameter also cannot read
    /// structured tool messages in its history.
    fn flatten_message(message: &Message) -> Message {
        let needs_flattening = message.content.iter().any(|b| {
            matches!(
                b,
                ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
            )
        });
        if !needs_flattening {
            return message.clone();
        }

        let mut text = String::new();
        let mut passthrough = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::ToolUse { name, input, .. } => {
                    let call = serde_json::json!({ "name": name, "input": input });
                    text.push_str(&format!("{OPEN}\n{call}\n{CLOSE}\n"));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let label = if *is_error {
                        "tool_error"
                    } else {
                        "tool_result"
                    };
                    text.push_str(&format!("<{label}>\n{content}\n</{label}>\n"));
                }
                ContentBlock::Text { text: t } => {
                    text.push_str(t);
                    text.push('\n');
                }
                // Thinking is dropped: replaying it as plain text invites the
                // model to imitate the format instead of the behavior.
                ContentBlock::Thinking { .. } => {}
                other => passthrough.push(other.clone()),
            }
        }

        let mut content = Vec::new();
        if !text.trim().is_empty() {
            content.push(ContentBlock::text(text.trim_end().to_string()));
        }
        content.extend(passthrough);

        // Tool results arrive on a user-role message already; a flattened
        // assistant turn stays assistant.
        Message::new(message.role, content)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Text,
    InCall,
}

/// Incremental scanner converting a prompted model's raw text stream into
/// [`StreamEvent`]s.
///
/// Feed it every text delta in order and forward whatever it returns. It holds
/// back the minimum tail needed to recognize a marker split across chunk
/// boundaries, so `<tool` / `_call>` arriving in separate deltas still works.
#[derive(Debug)]
pub struct PromptedScanner {
    buf: String,
    state: State,
    saw_tool_call: bool,
    next_id: u32,
}

impl Default for PromptedScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PromptedScanner {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            state: State::Text,
            saw_tool_call: false,
            next_id: 0,
        }
    }

    /// True once at least one tool call has been parsed, which is how the
    /// adapter decides between `StopReason::ToolUse` and `EndTurn`.
    pub fn saw_tool_call(&self) -> bool {
        self.saw_tool_call
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<StreamEvent> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        loop {
            match self.state {
                State::Text => match self.buf.find(OPEN) {
                    Some(i) => {
                        let head: String = self.buf.drain(..i).collect();
                        self.buf.drain(..OPEN.len());
                        Self::emit_text(&mut out, &head);
                        self.state = State::InCall;
                    }
                    None => {
                        // Release everything that cannot still be the start of
                        // an opening marker.
                        let keep = safe_tail(&self.buf, OPEN.len() - 1);
                        let split = self.buf.len() - keep;
                        if split > 0 {
                            let head: String = self.buf.drain(..split).collect();
                            Self::emit_text(&mut out, &head);
                        }
                        break;
                    }
                },
                State::InCall => match self.buf.find(CLOSE) {
                    Some(i) => {
                        let payload: String = self.buf.drain(..i).collect();
                        self.buf.drain(..CLOSE.len());
                        out.extend(self.emit_call(&payload));
                        self.state = State::Text;
                    }
                    None => break,
                },
            }
        }
        out
    }

    /// Flushes buffered content at end of stream. An unterminated `<tool_call>`
    /// is still parsed: models routinely omit the closing tag when they hit a
    /// stop sequence or token limit right after the JSON.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let rest: String = std::mem::take(&mut self.buf);
        match self.state {
            State::Text => Self::emit_text(&mut out, &rest),
            State::InCall if !rest.trim().is_empty() => out.extend(self.emit_call(&rest)),
            State::InCall => {}
        }
        self.state = State::Text;
        out
    }

    fn emit_text(out: &mut Vec<StreamEvent>, text: &str) {
        if !text.is_empty() {
            out.push(StreamEvent::TextDelta { text: text.into() });
        }
    }

    fn emit_call(&mut self, payload: &str) -> Vec<StreamEvent> {
        self.saw_tool_call = true;
        self.next_id += 1;
        let id = format!("ptc_{}", self.next_id);

        let parsed: Option<serde_json::Value> = serde_json::from_str(payload.trim()).ok();
        let (name, input) = match parsed.as_ref().and_then(|v| v.as_object()) {
            Some(obj) => {
                // Accept `tool` as an alias for `name`, and a missing `input`
                // as an empty one. Both are common near-misses.
                let name = obj
                    .get("name")
                    .or_else(|| obj.get("tool"))
                    .and_then(|v| v.as_str());
                let input = obj
                    .get("input")
                    .or_else(|| obj.get("arguments"))
                    .or_else(|| obj.get("parameters"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                match name {
                    Some(n) => (n.to_string(), input),
                    None => (MALFORMED_TOOL.to_string(), serde_json::json!(payload)),
                }
            }
            None => (MALFORMED_TOOL.to_string(), serde_json::json!(payload)),
        };

        vec![
            StreamEvent::ToolUseStart {
                id: id.clone(),
                name,
            },
            StreamEvent::ToolUseInputDelta {
                id: id.clone(),
                json: input.to_string(),
            },
            StreamEvent::ToolUseEnd { id },
        ]
    }
}

/// Length in bytes of the tail to retain, at most `max`, snapped down to a char
/// boundary so the buffer is never split mid-codepoint.
fn safe_tail(s: &str, max: usize) -> usize {
    let mut n = max.min(s.len());
    while n > 0 && !s.is_char_boundary(s.len() - n) {
        n -= 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;
    use crate::stream::StreamAccumulator;

    fn drive(chunks: &[&str]) -> (Vec<StreamEvent>, bool) {
        let mut scanner = PromptedScanner::new();
        let mut events = Vec::new();
        for c in chunks {
            events.extend(scanner.feed(c));
        }
        events.extend(scanner.finish());
        (events, scanner.saw_tool_call())
    }

    fn collect(chunks: &[&str]) -> Message {
        let (events, _) = drive(chunks);
        let mut acc = StreamAccumulator::new();
        for e in events {
            acc.push(e);
        }
        acc.finish().0
    }

    #[test]
    fn plain_text_passes_through_untouched() {
        let msg = collect(&["Hello ", "world"]);
        assert_eq!(msg.text(), "Hello world");
        assert!(!msg.has_tool_use());
    }

    #[test]
    fn parses_a_tool_call() {
        let msg = collect(&[
            "Let me look.\n<tool_call>\n{\"name\": \"read_file\", \"input\": {\"path\": \"a.txt\"}}\n</tool_call>",
        ]);
        assert_eq!(msg.text().trim(), "Let me look.");
        let (_, name, input) = msg.tool_uses().next().unwrap();
        assert_eq!(name, "read_file");
        assert_eq!(input, &serde_json::json!({"path": "a.txt"}));
    }

    #[test]
    fn marker_split_across_chunks_still_matches() {
        // The exact failure a naive per-chunk `contains` check would miss.
        let msg = collect(&[
            "ok <tool",
            "_call>",
            "\n{\"name\":\"list_dir\",\"input\":{}}\n</tool",
            "_call>",
        ]);
        assert_eq!(msg.text().trim(), "ok");
        assert_eq!(msg.tool_uses().next().unwrap().1, "list_dir");
    }

    #[test]
    fn json_split_across_chunks_still_matches() {
        let msg = collect(&[
            "<tool_call>{\"name\":\"grep\",",
            "\"input\":{\"pattern\":",
            "\"fn main\"}}</tool_call>",
        ]);
        let (_, name, input) = msg.tool_uses().next().unwrap();
        assert_eq!(name, "grep");
        assert_eq!(input, &serde_json::json!({"pattern": "fn main"}));
    }

    #[test]
    fn handles_several_calls_in_one_turn() {
        let msg = collect(&[
            "<tool_call>{\"name\":\"a\",\"input\":{}}</tool_call>",
            "between",
            "<tool_call>{\"name\":\"b\",\"input\":{}}</tool_call>",
        ]);
        let names: Vec<_> = msg.tool_uses().map(|(_, n, _)| n).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert_eq!(msg.text(), "between");
    }

    #[test]
    fn accepts_arguments_and_tool_aliases() {
        let msg = collect(&["<tool_call>{\"tool\":\"x\",\"arguments\":{\"k\":1}}</tool_call>"]);
        let (_, name, input) = msg.tool_uses().next().unwrap();
        assert_eq!(name, "x");
        assert_eq!(input, &serde_json::json!({"k": 1}));
    }

    #[test]
    fn missing_input_becomes_empty_object() {
        let msg = collect(&["<tool_call>{\"name\":\"list_dir\"}</tool_call>"]);
        assert_eq!(msg.tool_uses().next().unwrap().2, &serde_json::json!({}));
    }

    #[test]
    fn unterminated_call_at_end_of_stream_is_still_parsed() {
        let msg = collect(&["<tool_call>\n{\"name\":\"a\",\"input\":{}}"]);
        assert_eq!(msg.tool_uses().next().unwrap().1, "a");
    }

    #[test]
    fn malformed_json_yields_a_recoverable_call_not_a_dropped_one() {
        let (events, saw) = drive(&["<tool_call>{{{ not json </tool_call>"]);
        assert!(saw);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolUseStart { name, .. } if name == MALFORMED_TOOL
        ));
    }

    #[test]
    fn json_fence_in_prose_is_not_mistaken_for_a_call() {
        let msg = collect(&["Here is config:\n```json\n{\"name\": \"read_file\"}\n```\ndone"]);
        assert!(!msg.has_tool_use());
        assert!(msg.text().contains("```json"));
    }

    #[test]
    fn multibyte_text_is_never_split_mid_codepoint() {
        let msg = collect(&["日本語のテキスト", "とえもじ🎉です"]);
        assert_eq!(msg.text(), "日本語のテキストとえもじ🎉です");
    }

    #[test]
    fn rewrite_moves_tools_into_system_and_flattens_history() {
        let mut req = ChatRequest::new(
            "gemma3",
            vec![
                Message::user("hi"),
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::tool_use(
                        "t1",
                        "read_file",
                        serde_json::json!({"path": "a.txt"}),
                    )],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::tool_result("t1", "file contents")],
                ),
            ],
        )
        .with_system("You are Taurus.")
        .with_tools(vec![ToolDef {
            name: "read_file".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }]);

        PromptedTools::rewrite(&mut req);

        assert!(req.tools.is_empty());
        let system = req.system.as_deref().unwrap();
        assert!(system.starts_with("You are Taurus."));
        assert!(system.contains("read_file"));
        assert!(system.contains(OPEN));

        // Round trip: the flattened history must re-scan into the same call.
        let replayed = req.messages[1].text();
        assert!(replayed.contains("read_file"));
        assert!(req.messages[2].text().contains("file contents"));
        assert!(req.messages.iter().all(|m| !m.has_tool_use()));
    }

    #[test]
    fn rewrite_is_a_no_op_without_tools() {
        let mut req = ChatRequest::new("gemma3", vec![Message::user("hi")]).with_system("S");
        PromptedTools::rewrite(&mut req);
        assert_eq!(req.system.as_deref(), Some("S"));
    }
}
