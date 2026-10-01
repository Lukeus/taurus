//! The Responses API: `/v1/responses`.
//!
//! OpenAI's newer route, and on its reasoning models the only one that takes
//! function tools and reasoning in the same request. Chat completions refuses
//! the pair outright on those models, which leaves a harness that only speaks
//! chat completions choosing between tools and thinking.
//!
//! The shape is items rather than messages. A turn's reasoning, its text and
//! each tool call are separate entries in one flat `input` list, and the order
//! they arrived in is the order they go back in. That ordering is the point:
//! a reasoning item has to sit in front of the call it led to, or the model
//! reads its own history as a call it made without thinking.
//!
//! Nothing is stored server-side. Every request carries `store: false`, so a
//! conversation's history lives here the way it does for every other backend,
//! and the reasoning comes back encrypted for this harness to replay. That's
//! also what an organization with zero data retention requires.

use std::collections::HashMap;

use serde_json::{json, Value};
use taurus_provider::{
    relocated_note, ChatRequest, ContentBlock, StreamEvent, TokenUsage, ToolDef,
    OPENAI_REASONING_PREFIX,
};

/// What to ask of the model's reasoning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reasoning {
    /// Send nothing about it. For a model that has refused the parameters,
    /// because it doesn't reason at all.
    Omit,
    /// Ask for a summary and the encrypted form, at the model's own effort.
    Default,
    /// The same, at the effort the config named.
    Effort(String),
}

/// The request body.
pub fn body(request: &ChatRequest, reasoning: &Reasoning) -> Value {
    let mut body = json!({
        "model": request.model,
        "input": input_items(request),
        "stream": true,
        // Kept here, not there: every other backend's history lives in this
        // harness, and one conversation shouldn't be split between two places.
        "store": false,
    });
    if let Some(system) = request.system.as_ref().filter(|s| !s.trim().is_empty()) {
        body["instructions"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = json!(tools(&request.tools));
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(max) = request.max_tokens {
        body["max_output_tokens"] = json!(max);
    }
    // No `stop`: this route doesn't take one. The adapter enforces the
    // request's stop sequences on the text itself, in `Stopper`.

    let (effort, summarized) = match reasoning {
        Reasoning::Omit => return body,
        Reasoning::Default => (None, true),
        // `none` is a model that doesn't reason this turn, so there's no
        // summary to ask for and nothing encrypted to carry.
        Reasoning::Effort(effort) if effort == "none" => (Some(effort), false),
        Reasoning::Effort(effort) => (Some(effort), true),
    };
    let mut config = serde_json::Map::new();
    if let Some(effort) = effort {
        config.insert("effort".into(), json!(effort));
    }
    if summarized {
        config.insert("summary".into(), json!("auto"));
        // The reasoning itself, sealed. With `store: false` it's the only way
        // the next request can carry what this one thought, and a reasoning
        // model that loses it between tool calls re-derives its plan each time.
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    body["reasoning"] = Value::Object(config);
    body
}

fn tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                // Flat, not nested under `function` the way chat completions
                // has it.
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.input_schema,
                // The schemas here aren't written for strict mode, which
                // requires every property listed as required and no extra
                // ones allowed. Turned on, it rejects most of them.
                "strict": false,
            })
        })
        .collect()
}

/// Flattens the conversation into the route's item list.
///
/// Each message's blocks go out in the order they arrived, because the order
/// carries meaning here: reasoning, then the call it led to. The one
/// exception is tool results, which lead their message for the reason chat
/// completions has: an answer has to follow its call with nothing between.
pub fn input_items(request: &ChatRequest) -> Vec<Value> {
    let names_by_id: HashMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|m| m.tool_uses())
        .map(|(id, name, _)| (id, name))
        .collect();

    let mut out = Vec::new();
    for message in &request.messages {
        let role = message.role.as_str();
        let assistant = role == "assistant";
        let mut results = Vec::new();
        let mut items = Vec::new();
        // Text and images collect into one message item until something that
        // isn't a message — reasoning, a call — has to come between.
        let mut parts: Vec<Value> = Vec::new();
        let flush = |parts: &mut Vec<Value>, items: &mut Vec<Value>| {
            if !parts.is_empty() {
                items.push(json!({
                    "type": "message",
                    "role": role,
                    "content": std::mem::take(parts),
                }));
            }
        };

        for block in &message.content {
            match block {
                ContentBlock::Text { text } if text.is_empty() => {}
                ContentBlock::Text { text } => parts.push(if assistant {
                    json!({ "type": "output_text", "text": text })
                } else {
                    json!({ "type": "input_text", "text": text })
                }),
                ContentBlock::Image { mime_type, data } => parts.push(json!({
                    "type": "input_image",
                    "image_url": format!("data:{mime_type};base64,{data}"),
                })),
                // Only reasoning this route issued, which is the only kind it
                // can decrypt. Anything else — unsigned, or signed by another
                // provider earlier in the conversation — would be a 400.
                ContentBlock::Thinking { text, signature } => {
                    let Some(encrypted) = signature
                        .as_deref()
                        .and_then(|s| s.strip_prefix(OPENAI_REASONING_PREFIX))
                    else {
                        continue;
                    };
                    flush(&mut parts, &mut items);
                    let summary = if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![json!({ "type": "summary_text", "text": text })]
                    };
                    // No `id`: with `store: false` there's nothing on the
                    // server for one to point at, and naming it asks the API to
                    // look it up and fail.
                    items.push(json!({
                        "type": "reasoning",
                        "summary": summary,
                        "encrypted_content": encrypted,
                    }));
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    flush(&mut parts, &mut items);
                    items.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": input.to_string(),
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let (body, relocated) = content.split_relocating_images();
                    let body = if *is_error {
                        format!("Error: {body}")
                    } else {
                        body
                    };
                    results.push(json!({
                        "type": "function_call_output",
                        "call_id": tool_use_id,
                        "output": body,
                    }));
                    if !relocated.is_empty() {
                        let mut content = vec![json!({
                            "type": "input_text",
                            "text": relocated_note(
                                names_by_id.get(tool_use_id.as_str()).copied(),
                                relocated.len(),
                            ),
                        })];
                        content.extend(relocated.iter().map(|(mime_type, data)| {
                            json!({
                                "type": "input_image",
                                "image_url": format!("data:{mime_type};base64,{data}"),
                            })
                        }));
                        results.push(json!({
                            "type": "message",
                            "role": "user",
                            "content": content,
                        }));
                    }
                }
            }
        }
        flush(&mut parts, &mut items);
        out.extend(results);
        out.extend(items);
    }
    out
}

/// What one decoded event asks the adapter to do.
#[derive(Debug, PartialEq)]
pub enum Piece {
    /// Answer text, which has to pass the stop sequences and, on a prompted
    /// model, the tool-call scanner before it's an event.
    Text(String),
    Event(StreamEvent),
}

/// How the response ended, when it said.
#[derive(Debug, PartialEq)]
pub enum Ending {
    Completed,
    /// Cut short. `max_output_tokens` is the one reason that maps to a stop
    /// reason; the other, a content filter, ends the turn like any answer.
    Incomplete(Option<String>),
    Failed(String),
}

/// Turns the route's typed events into the harness's.
///
/// Keyed by output index rather than item id: every event carries the index,
/// and not every server that imitates this route sends ids on the deltas.
#[derive(Debug, Default)]
pub struct Decoder {
    /// Calls opened and not yet closed, by output index: the call id, and
    /// whether any arguments have been sent for it yet.
    calls: HashMap<u64, (String, bool)>,
    saw_call: bool,
    usage: TokenUsage,
    ending: Option<Ending>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// One SSE `data:` payload in, the pieces it means out.
    pub fn feed(&mut self, event: &Value) -> Vec<Piece> {
        let index = event["output_index"].as_u64().unwrap_or(0);
        let delta = || event["delta"].as_str().unwrap_or_default().to_string();
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" | "response.refusal.delta" => {
                let text = delta();
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![Piece::Text(text)]
                }
            }
            // A summary arrives in parts; the break between two is a
            // paragraph, which is how they read on the API's own console.
            "response.reasoning_summary_part.added"
                if event["summary_index"].as_u64().unwrap_or(0) > 0 =>
            {
                vec![Piece::Event(StreamEvent::ThinkingDelta {
                    text: "\n\n".into(),
                })]
            }
            // The summary on OpenAI's own models; the raw text on the
            // open-weight ones a self-hosted server runs behind this route.
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                vec![Piece::Event(StreamEvent::ThinkingDelta { text: delta() })]
            }
            "response.output_item.added" => self.item_added(index, &event["item"]),
            "response.function_call_arguments.delta" => {
                let json = delta();
                match self.calls.get_mut(&index) {
                    Some((id, sent)) if !json.is_empty() => {
                        *sent = true;
                        vec![Piece::Event(StreamEvent::ToolUseInputDelta {
                            id: id.clone(),
                            json,
                        })]
                    }
                    _ => Vec::new(),
                }
            }
            "response.output_item.done" => self.item_done(index, &event["item"]),
            "response.completed" => {
                self.read_usage(&event["response"]);
                self.ending = Some(Ending::Completed);
                Vec::new()
            }
            "response.incomplete" => {
                self.read_usage(&event["response"]);
                self.ending = Some(Ending::Incomplete(
                    event["response"]["incomplete_details"]["reason"]
                        .as_str()
                        .map(str::to_string),
                ));
                Vec::new()
            }
            "response.failed" => {
                self.read_usage(&event["response"]);
                self.ending = Some(Ending::Failed(failure(&event["response"]["error"])));
                Vec::new()
            }
            // A failure outside any response, which is how this route reports
            // one that happened before the response object existed.
            "error" => {
                self.ending = Some(Ending::Failed(failure(event)));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn item_added(&mut self, index: u64, item: &Value) -> Vec<Piece> {
        match item["type"].as_str().unwrap_or_default() {
            "function_call" => {
                let id = item["call_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(taurus_provider::new_tool_use_id);
                let name = item["name"].as_str().unwrap_or_default().to_string();
                self.calls.insert(index, (id.clone(), false));
                self.saw_call = true;
                vec![Piece::Event(StreamEvent::ToolUseStart { id, name })]
            }
            // Opened before any text arrives. A model can reason and return no
            // summary at all, and its encrypted reasoning still has to land on
            // a block, or the signature has nothing to attach to and the next
            // request goes out without it.
            "reasoning" => vec![Piece::Event(StreamEvent::ThinkingDelta {
                text: String::new(),
            })],
            _ => Vec::new(),
        }
    }

    fn item_done(&mut self, index: u64, item: &Value) -> Vec<Piece> {
        match item["type"].as_str().unwrap_or_default() {
            "function_call" => {
                let Some((id, sent)) = self.calls.remove(&index) else {
                    return Vec::new();
                };
                let mut pieces = Vec::new();
                // A server that sends a call whole, with no argument deltas,
                // still has it in the finished item.
                if !sent {
                    if let Some(args) = item["arguments"].as_str().filter(|a| !a.is_empty()) {
                        pieces.push(Piece::Event(StreamEvent::ToolUseInputDelta {
                            id: id.clone(),
                            json: args.to_string(),
                        }));
                    }
                }
                pieces.push(Piece::Event(StreamEvent::ToolUseEnd { id }));
                pieces
            }
            "reasoning" => match item["encrypted_content"].as_str() {
                Some(sealed) if !sealed.is_empty() => {
                    vec![Piece::Event(StreamEvent::ThinkingSignature {
                        signature: format!("{OPENAI_REASONING_PREFIX}{sealed}"),
                    })]
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn read_usage(&mut self, response: &Value) {
        let usage = &response["usage"];
        if usage.is_null() {
            return;
        }
        let count = |v: &Value| v.as_u64().map(|n| n as u32);
        self.usage = TokenUsage {
            input_tokens: count(&usage["input_tokens"]).unwrap_or(0),
            // Includes the reasoning, which is also reported on its own below.
            // Adding the two would bill the reasoning twice.
            output_tokens: count(&usage["output_tokens"]).unwrap_or(0),
            cache_read_input_tokens: count(&usage["input_tokens_details"]["cached_tokens"]),
            cache_creation_input_tokens: None,
            reasoning_tokens: count(&usage["output_tokens_details"]["reasoning_tokens"]),
        };
    }

    /// Calls opened and never closed, which a stream that ended early leaves
    /// behind. Closed so the accumulator parses whatever input they got.
    pub fn unclosed(&mut self) -> Vec<StreamEvent> {
        self.calls
            .drain()
            .map(|(_, (id, _))| StreamEvent::ToolUseEnd { id })
            .collect()
    }

    pub fn saw_call(&self) -> bool {
        self.saw_call
    }

    pub fn usage(&self) -> TokenUsage {
        self.usage
    }

    pub fn ending(&self) -> Option<&Ending> {
        self.ending.as_ref()
    }
}

fn failure(error: &Value) -> String {
    let message = error["message"].as_str().unwrap_or("no reason given");
    match error["code"].as_str().filter(|c| !c.is_empty()) {
        Some(code) => format!("{message} ({code})"),
        None => message.to_string(),
    }
}

/// The request's stop sequences, enforced on the text as it streams.
///
/// This route has no `stop` parameter, and the prompted-tool fallback depends
/// on one: it's what keeps a model from writing the result of the call it just
/// made. So the adapter does what the server would have: it holds back
/// anything that might be the start of a stop sequence until the next delta
/// settles it, and ends the answer at the first one.
#[derive(Debug, Default)]
pub struct Stopper {
    stops: Vec<String>,
    held: String,
}

impl Stopper {
    pub fn new(stops: &[String]) -> Self {
        Self {
            stops: stops.iter().filter(|s| !s.is_empty()).cloned().collect(),
            held: String::new(),
        }
    }

    /// The text that's safe to pass on, and whether a stop sequence ended it.
    pub fn feed(&mut self, delta: &str) -> (String, bool) {
        if self.stops.is_empty() {
            return (delta.to_string(), false);
        }
        self.held.push_str(delta);
        if let Some(at) = self.stops.iter().filter_map(|s| self.held.find(s)).min() {
            let text = self.held[..at].to_string();
            self.held.clear();
            return (text, true);
        }
        // Keep the longest tail that could still grow into a stop sequence.
        let keep = self
            .held
            .char_indices()
            .map(|(i, _)| i)
            .find(|&i| {
                let tail = &self.held[i..];
                self.stops.iter().any(|s| s.starts_with(tail))
            })
            .unwrap_or(self.held.len());
        let text = self.held[..keep].to_string();
        self.held.drain(..keep);
        (text, false)
    }

    /// Whatever was held back, once the stream has ended without completing it.
    pub fn finish(&mut self) -> String {
        std::mem::take(&mut self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_provider::{Message, Role};

    fn assistant(blocks: Vec<ContentBlock>) -> ChatRequest {
        ChatRequest::new("m", vec![Message::new(Role::Assistant, blocks)])
    }

    #[test]
    fn reasoning_goes_back_in_front_of_the_call_it_led_to() {
        let request = assistant(vec![
            ContentBlock::Thinking {
                text: "need the file".into(),
                signature: Some(format!("{OPENAI_REASONING_PREFIX}sealed-1")),
            },
            ContentBlock::tool_use("call_1", "read_file", json!({"path": "a.rs"})),
        ]);
        let items = input_items(&request);
        assert_eq!(items[0]["type"], "reasoning");
        assert_eq!(items[0]["encrypted_content"], "sealed-1");
        assert_eq!(items[0]["summary"][0]["text"], "need the file");
        assert!(items[0].get("id").is_none(), "{items:#?}");
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(
            serde_json::from_str::<Value>(items[1]["arguments"].as_str().unwrap()).unwrap(),
            json!({"path": "a.rs"})
        );
    }

    #[test]
    fn reasoning_another_provider_signed_is_left_out() {
        // A conversation that started on Anthropic and moved here. Its
        // signature isn't something this API can decrypt, and sending it is a
        // 400 on every request for the rest of the conversation.
        let request = assistant(vec![
            ContentBlock::Thinking {
                text: "elsewhere".into(),
                signature: Some("anthropic-sig".into()),
            },
            ContentBlock::thinking("never signed"),
            ContentBlock::text("answer"),
        ]);
        let items = input_items(&request);
        assert_eq!(items.len(), 1, "{items:#?}");
        assert_eq!(items[0]["role"], "assistant");
        assert_eq!(items[0]["content"][0]["type"], "output_text");
    }

    #[test]
    fn a_tool_result_is_an_output_item_keyed_by_call_id() {
        let request = ChatRequest::new(
            "m",
            vec![Message::new(
                Role::User,
                vec![
                    ContentBlock::text("and then"),
                    ContentBlock::tool_result("call_1", "file body"),
                ],
            )],
        );
        let items = input_items(&request);
        // The result first, though it came second: nothing may sit between a
        // call and its answer.
        assert_eq!(items[0]["type"], "function_call_output");
        assert_eq!(items[0]["call_id"], "call_1");
        assert_eq!(items[0]["output"], "file body");
        assert_eq!(items[1]["content"][0]["type"], "input_text");
    }

    #[test]
    fn the_system_prompt_is_instructions_not_an_item() {
        let request = ChatRequest::new("m", vec![Message::user("hi")]).with_system("S");
        let body = body(&request, &Reasoning::Default);
        assert_eq!(body["instructions"], "S");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert_eq!(body["store"], false);
    }

    #[test]
    fn tools_are_flat_and_not_strict() {
        let request = ChatRequest::new("m", vec![Message::user("hi")]).with_tools(vec![ToolDef {
            name: "read_file".into(),
            description: "Reads.".into(),
            input_schema: json!({"type": "object"}),
        }]);
        let body = body(&request, &Reasoning::Default);
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["strict"], false);
        assert!(body["tools"][0].get("function").is_none());
    }

    #[test]
    fn reasoning_settings_reach_the_body() {
        let request = ChatRequest::new("m", vec![Message::user("hi")]);

        let default = body(&request, &Reasoning::Default);
        assert_eq!(default["reasoning"], json!({"summary": "auto"}));
        assert_eq!(default["include"], json!(["reasoning.encrypted_content"]));

        let high = body(&request, &Reasoning::Effort("high".into()));
        assert_eq!(
            high["reasoning"],
            json!({"effort": "high", "summary": "auto"})
        );

        // Nothing to summarize and nothing to carry.
        let none = body(&request, &Reasoning::Effort("none".into()));
        assert_eq!(none["reasoning"], json!({"effort": "none"}));
        assert!(none.get("include").is_none());

        let omitted = body(&request, &Reasoning::Omit);
        assert!(omitted.get("reasoning").is_none());
        assert!(omitted.get("include").is_none());
    }

    #[test]
    fn a_stop_sequence_split_across_deltas_still_stops() {
        let mut stopper = Stopper::new(&["<tool_result>".into()]);
        assert_eq!(
            stopper.feed("called it <tool"),
            ("called it ".into(), false)
        );
        assert_eq!(stopper.feed("_result>made up"), (String::new(), true));
    }

    #[test]
    fn text_that_only_looked_like_a_stop_is_released() {
        let mut stopper = Stopper::new(&["<tool_result>".into()]);
        assert_eq!(stopper.feed("a <to"), ("a ".into(), false));
        assert_eq!(stopper.feed("p> b"), ("<top> b".into(), false));
        assert_eq!(stopper.feed("<"), (String::new(), false));
        assert_eq!(stopper.finish(), "<");
    }

    #[test]
    fn no_stop_sequences_passes_everything_through() {
        let mut stopper = Stopper::new(&[]);
        assert_eq!(
            stopper.feed("<tool_result>"),
            ("<tool_result>".into(), false)
        );
    }

    #[test]
    fn the_earliest_of_two_stops_wins() {
        let mut stopper = Stopper::new(&["<b>".into(), "<a>".into()]);
        assert_eq!(stopper.feed("x<a>y<b>"), ("x".into(), true));
    }

    #[test]
    fn a_held_tail_never_splits_a_character() {
        let mut stopper = Stopper::new(&["é!".into()]);
        assert_eq!(stopper.feed("café"), ("caf".into(), false));
        assert_eq!(stopper.finish(), "é");
    }
}
