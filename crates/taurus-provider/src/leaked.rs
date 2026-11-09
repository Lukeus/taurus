//! Tool calls a model wrote as text when it had a real tools parameter.
//!
//! The commonest way a small local model fails as an agent isn't a wrong
//! answer. It's a right call written in the wrong place: offered tools through
//! the API, it writes `{"name": "read_file", "arguments": {...}}` as its reply,
//! or wraps it in a ```json fence, or in the `<tool_call>` tags its fine-tune
//! was trained on. The backend's tool-call parser misses it, the reply arrives
//! as prose, and the turn ends with the model waiting for a result nobody is
//! going to send.
//!
//! [`LeakGuard`] sits on the stream between the adapter and the agent loop and
//! turns those back into calls. It recognizes two shapes:
//!
//! - **The whole reply is the call or calls.** Bare JSON, an array of calls,
//!   one or more fenced blocks, or the `<|python_tag|>` and `[TOOL_CALLS]`
//!   prefixes Llama and Mistral templates use. Each call has to name a tool the
//!   request offered, and carry nothing but a name and arguments, so a reply
//!   that is a `package.json` stays a reply.
//! - **A `<tool_call>` block anywhere,** with prose around it. That tag means
//!   one thing, so it goes through [`PromptedScanner`] the way a prompted
//!   model's output does.
//!
//! Prose followed by a fenced call is *not* recognized. Catching it would mean
//! holding back every code block in every answer until the reply ends, to see
//! whether anything came after it.
//!
//! The text of a candidate reply is held, not streamed, until the guard knows
//! which it is. That's the cost: an answer that opens with JSON appears all at
//! once instead of as it's written. An answer that opens with anything else
//! streams exactly as before, because the decision is made on its first
//! non-blank characters.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::prompted::PromptedScanner;
use crate::repair::{repair_json, strip_fence};
use crate::stream::StreamEvent;

/// How much text is held before the guard gives up and streams it as prose.
/// A leaked `write_file` carries the file, so this is sized for a file.
const HOLD_LIMIT: usize = 64 * 1024;

/// What a chat template puts before a call when the backend doesn't strip it.
const PREFIXES: [&str; 2] = ["<|python_tag|>", "[TOOL_CALLS]"];

/// Fence languages a call is written under. Anything else is code.
const FENCE_LANGS: [&str; 5] = ["", "json", "jsonc", "tool_call", "tool_code"];

/// The keys a call object may carry. Anything else and it's data, not a call.
const NAME_KEYS: [&str; 3] = ["name", "tool", "tool_name"];
const ARG_KEYS: [&str; 4] = ["arguments", "parameters", "input", "args"];
const OTHER_KEYS: [&str; 2] = ["type", "id"];

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// Nothing but whitespace or the start of an opener has arrived.
    Undecided,
    /// The reply opens like a call, and is held until it ends.
    Holding,
    /// The reply is prose. `<tool_call>` blocks are still caught.
    Passing,
}

/// See the module docs.
#[derive(Debug)]
pub struct LeakGuard {
    names: HashSet<String>,
    mode: Mode,
    buf: String,
    scanner: PromptedScanner,
    recovered: usize,
}

impl LeakGuard {
    /// A guard for a request that offered these tools.
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: names.into_iter().collect(),
            mode: Mode::Undecided,
            buf: String::new(),
            scanner: PromptedScanner::new(),
            recovered: 0,
        }
    }

    /// How many calls this guard turned from text back into calls.
    pub fn recovered(&self) -> usize {
        self.recovered + usize::from(self.scanner.saw_tool_call())
    }

    /// Passes one event through, returning what to forward in its place.
    pub fn push(&mut self, event: StreamEvent) -> Vec<StreamEvent> {
        match event {
            StreamEvent::TextDelta { text } => self.feed(&text),
            // Says nothing about which block is open, so it can overtake held
            // text without changing the message.
            StreamEvent::Usage { .. } => vec![event],
            // Anything else opens or signs a block of its own, and the held
            // text came first. A real call arriving also settles the question:
            // the model can call tools, and what it wrote first was prose.
            // The scanner's own held tail, kept in case it was the start of a
            // tag, has to go out ahead of it as well.
            other => {
                let mut out = self.release();
                out.extend(self.scanner.finish());
                out.push(other);
                out
            }
        }
    }

    /// Everything still held, as calls if it was calls and as text if not.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        match self.mode {
            Mode::Undecided | Mode::Holding => {
                let held = std::mem::take(&mut self.buf);
                match parse_whole(&held, &self.names) {
                    Some(calls) => {
                        self.recovered += calls.len();
                        self.mode = Mode::Passing;
                        calls.into_iter().flat_map(call_events).collect()
                    }
                    None => {
                        self.mode = Mode::Passing;
                        let mut out = self.scanner.feed(&held);
                        out.extend(self.scanner.finish());
                        out
                    }
                }
            }
            Mode::Passing => self.scanner.finish(),
        }
    }

    /// What to show of a stream that failed: the prose, and nothing held as a
    /// possible call. A half-written call shown as text would make an attempt
    /// that can still be retried look like one that already said something.
    pub fn abandon(&mut self) -> Vec<StreamEvent> {
        match self.mode {
            Mode::Undecided | Mode::Holding => {
                self.buf.clear();
                Vec::new()
            }
            Mode::Passing => self
                .scanner
                .finish()
                .into_iter()
                .filter(|e| matches!(e, StreamEvent::TextDelta { .. }))
                .collect(),
        }
    }

    fn feed(&mut self, text: &str) -> Vec<StreamEvent> {
        match self.mode {
            Mode::Passing => self.scanner.feed(text),
            Mode::Undecided | Mode::Holding => {
                self.buf.push_str(text);
                self.mode = classify(&self.buf);
                if self.mode == Mode::Passing || self.buf.len() > HOLD_LIMIT {
                    self.release()
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Stops holding: what was held goes on as prose, and so does what follows.
    fn release(&mut self) -> Vec<StreamEvent> {
        self.mode = Mode::Passing;
        let held = std::mem::take(&mut self.buf);
        if held.is_empty() {
            Vec::new()
        } else {
            self.scanner.feed(&held)
        }
    }
}

/// Whether a reply that begins with `text` could still be nothing but calls.
fn classify(text: &str) -> Mode {
    let t = text.trim_start();
    if t.is_empty() {
        return Mode::Undecided;
    }
    for prefix in PREFIXES {
        if t.starts_with(prefix) {
            return Mode::Holding;
        }
        if prefix.starts_with(t) {
            return Mode::Undecided;
        }
    }
    if t.starts_with('{') {
        return Mode::Holding;
    }
    if let Some(rest) = t.strip_prefix('[') {
        // A Markdown link opens with `[` too. An array of calls opens `[{`.
        return match rest.trim_start().chars().next() {
            None => Mode::Undecided,
            Some('{') => Mode::Holding,
            Some(_) => Mode::Passing,
        };
    }
    if "```".starts_with(t) {
        return Mode::Undecided;
    }
    if let Some(rest) = t.strip_prefix("```") {
        // Decided on the info string, so a reply that opens with a Rust block
        // streams from its second line.
        return match rest.find('\n') {
            None if rest.len() > 16 => Mode::Passing,
            None => Mode::Undecided,
            Some(end) => {
                let lang = rest[..end].trim().to_ascii_lowercase();
                if FENCE_LANGS.contains(&lang.as_str()) {
                    Mode::Holding
                } else {
                    Mode::Passing
                }
            }
        };
    }
    Mode::Passing
}

/// The calls `text` consists of, if it consists of nothing else.
fn parse_whole(text: &str, names: &HashSet<String>) -> Option<Vec<(String, Value)>> {
    let mut t = text.trim();
    for prefix in PREFIXES {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest.trim_start();
        }
    }
    let mut bodies = Vec::new();
    if t.starts_with("```") {
        // One or more fenced blocks, with nothing but whitespace between.
        let mut rest = t;
        while !rest.is_empty() {
            let after_open = rest.strip_prefix("```")?;
            let close = after_open.find("```")?;
            let block = &rest[..close + 6];
            bodies.push(strip_fence(block)?);
            rest = after_open[close + 3..].trim_start();
        }
    } else {
        bodies.push(t);
    }

    let mut calls = Vec::new();
    for body in bodies {
        let mut values = serde_json::Deserializer::from_str(body).into_iter::<Value>();
        let mut any = false;
        for value in values.by_ref() {
            any = true;
            match value.ok()? {
                Value::Array(items) => {
                    for item in items {
                        calls.push(as_call(item, names)?);
                    }
                }
                other => calls.push(as_call(other, names)?),
            }
        }
        if !any {
            return None;
        }
    }
    (!calls.is_empty()).then_some(calls)
}

/// One call, from the shapes models write it in: `{name, arguments}` with its
/// aliases, and OpenAI's `{type: "function", function: {...}}` wrapper.
fn as_call(value: Value, names: &HashSet<String>) -> Option<(String, Value)> {
    let Value::Object(mut obj) = value else {
        return None;
    };
    if let Some(Value::Object(inner)) = obj.remove("function") {
        if obj.keys().any(|k| !OTHER_KEYS.contains(&k.as_str())) {
            return None;
        }
        obj = inner;
    }
    if obj.keys().any(|k| {
        let k = k.as_str();
        !NAME_KEYS.contains(&k) && !ARG_KEYS.contains(&k) && !OTHER_KEYS.contains(&k)
    }) {
        return None;
    }
    let name = NAME_KEYS
        .iter()
        .find_map(|k| obj.get(*k))
        .and_then(Value::as_str)?
        .to_string();
    if !names.contains(&name) {
        return None;
    }
    let input = match ARG_KEYS.iter().find_map(|k| obj.remove(*k)) {
        None => Value::Object(Map::new()),
        Some(Value::Object(args)) => Value::Object(args),
        // OpenAI's wire format sends arguments as a string of JSON, and a
        // model imitating it does too.
        Some(Value::String(s)) => repair_json(&s)?,
        Some(_) => return None,
    };
    Some((name, input))
}

fn call_events((name, input): (String, Value)) -> Vec<StreamEvent> {
    let id = crate::new_tool_use_id();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::stream::StreamAccumulator;
    use serde_json::json;

    fn guard() -> LeakGuard {
        LeakGuard::new(["read_file".to_string(), "list_dir".to_string()])
    }

    /// Streams `chunks` through a guard, returning the message and how many
    /// text deltas went out before the stream ended.
    fn run(chunks: &[&str]) -> (Message, usize, usize) {
        let mut g = guard();
        let mut acc = StreamAccumulator::new();
        let mut early = 0;
        for chunk in chunks {
            for e in g.push(StreamEvent::TextDelta {
                text: (*chunk).into(),
            }) {
                if matches!(e, StreamEvent::TextDelta { .. }) {
                    early += 1;
                }
                acc.push(e);
            }
        }
        for e in g.finish() {
            acc.push(e);
        }
        (acc.finish().0, early, g.recovered())
    }

    fn only_call(message: &Message) -> (String, Value) {
        let calls: Vec<_> = message.tool_uses().collect();
        assert_eq!(calls.len(), 1, "{message:?}");
        (calls[0].1.to_string(), calls[0].2.clone())
    }

    #[test]
    fn prose_streams_and_is_left_alone() {
        let (msg, early, recovered) = run(&["Hello ", "there, ", "world"]);
        assert_eq!(msg.text(), "Hello there, world");
        assert!(early > 0, "prose should stream, not be held");
        assert_eq!(recovered, 0);
    }

    #[test]
    fn a_bare_json_reply_becomes_a_call() {
        let (msg, early, recovered) = run(&[
            "{\"name\": \"read_file\", ",
            "\"arguments\": {\"path\": \"a.rs\"}}",
        ]);
        assert_eq!(
            only_call(&msg),
            ("read_file".into(), json!({"path": "a.rs"}))
        );
        assert_eq!(msg.text(), "");
        assert_eq!(early, 0);
        assert_eq!(recovered, 1);
    }

    #[test]
    fn a_fenced_reply_becomes_a_call() {
        let (msg, ..) = run(&[
            "``",
            "`json\n{\"name\":\"list_dir\",\"parameters\":{}}\n```\n",
        ]);
        assert_eq!(only_call(&msg), ("list_dir".into(), json!({})));
    }

    #[test]
    fn openai_shaped_calls_with_string_arguments_are_read() {
        let (msg, ..) = run(&[
            r#"[{"type":"function","function":{"name":"read_file","arguments":"{\"path\":\"b\"}"}}]"#,
        ]);
        assert_eq!(only_call(&msg), ("read_file".into(), json!({"path": "b"})));
    }

    #[test]
    fn template_prefixes_are_stripped() {
        let (msg, ..) = run(&["<|python_", "tag|>{\"name\":\"list_dir\",\"arguments\":{}}"]);
        assert_eq!(only_call(&msg).0, "list_dir");
        let (msg, ..) = run(&["[TOOL_CALLS][{\"name\":\"list_dir\",\"arguments\":{}}]"]);
        assert_eq!(only_call(&msg).0, "list_dir");
    }

    #[test]
    fn several_calls_in_a_row_are_all_kept() {
        let (msg, _, recovered) = run(&[
            "{\"name\":\"list_dir\",\"arguments\":{}}\n",
            "{\"name\":\"read_file\",\"arguments\":{\"path\":\"x\"}}",
        ]);
        assert_eq!(msg.tool_uses().count(), 2);
        assert_eq!(recovered, 2);
    }

    #[test]
    fn a_tag_inside_prose_is_still_a_call() {
        let (msg, early, _) = run(&[
            "Let me look.\n<tool_",
            "call>{\"name\":\"read_file\",\"input\":{\"path\":\"c\"}}</tool_call>",
        ]);
        assert!(early > 0);
        assert_eq!(msg.text().trim(), "Let me look.");
        assert_eq!(only_call(&msg).0, "read_file");
    }

    #[test]
    fn an_unknown_tool_name_stays_text() {
        let text = "{\"name\": \"delete_everything\", \"arguments\": {}}";
        let (msg, _, recovered) = run(&[text]);
        assert_eq!(msg.text(), text);
        assert!(!msg.has_tool_use());
        assert_eq!(recovered, 0);
    }

    #[test]
    fn json_that_is_data_stays_text() {
        // Has a `name` that happens to be a tool's, and other keys besides.
        let text = "{\"name\": \"read_file\", \"version\": \"1.0.0\", \"arguments\": {}}";
        let (msg, ..) = run(&[text]);
        assert!(!msg.has_tool_use());
        assert_eq!(msg.text(), text);
    }

    #[test]
    fn a_code_block_in_another_language_streams() {
        let (msg, early, _) = run(&["```rust\n", "fn main() {}\n", "```"]);
        assert!(early > 0);
        assert!(!msg.has_tool_use());
        assert_eq!(msg.text(), "```rust\nfn main() {}\n```");
    }

    #[test]
    fn a_markdown_link_streams() {
        let (_, early, _) = run(&["[the docs](", "https://x)"]);
        assert!(early > 0);
    }

    #[test]
    fn prose_after_a_call_shaped_start_keeps_everything_as_text() {
        let text = "{\"name\":\"list_dir\",\"arguments\":{}} is how you'd call it.";
        let (msg, ..) = run(&[text]);
        assert!(!msg.has_tool_use());
        assert_eq!(msg.text(), text);
    }

    #[test]
    fn a_real_call_releases_held_text_first() {
        let mut g = guard();
        assert!(g
            .push(StreamEvent::TextDelta {
                text: "{\"a\":".into()
            })
            .is_empty());
        let out = g.push(StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "list_dir".into(),
        });
        assert!(matches!(&out[0], StreamEvent::TextDelta { text } if text == "{\"a\":"));
        assert!(matches!(&out[1], StreamEvent::ToolUseStart { .. }));
        assert!(g.finish().is_empty());
        assert_eq!(g.recovered(), 0);
    }
}
