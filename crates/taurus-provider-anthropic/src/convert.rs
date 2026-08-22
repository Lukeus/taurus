//! Normalized blocks to Anthropic's Messages API shape.
//!
//! The shortest conversion in the harness, and deliberately so: the normalized
//! types were modelled on Anthropic content blocks in the first place, because
//! blocks are the superset that an OpenAI or Ollama response maps into without
//! loss. This file is mostly renaming fields.

use serde_json::{json, Value};
use taurus_provider::{ChatRequest, ContentBlock, Message, Role, ToolDef};

/// Marks a prefix worth caching on the way out.
///
/// Anthropic prices a cache read at about a tenth of a fresh read, and the part
/// of every request that repeats is exactly the part `taurus usage` reports as
/// fixed overhead — the system prompt and the tool schemas, re-sent on every
/// iteration of every turn. Two breakpoints out of the four allowed cover it:
/// one after the system prompt, which also covers the tools rendered before it,
/// and one on the last message, which extends the cached prefix by a turn each
/// time the conversation grows.
fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

/// The `tools` array, with the last one marked as a cache breakpoint.
pub fn tools_to_wire(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                // The one field that needs no translation at all: `ToolDef`
                // already carries a JSON Schema under the name Anthropic uses.
                "input_schema": t.input_schema,
            })
        })
        .collect()
}

/// The `system` field, as blocks so a cache breakpoint can sit on it.
///
/// Tools render before system, so a breakpoint here covers both. That is the
/// whole fixed overhead of a request in one entry.
pub fn system_to_wire(system: Option<&str>) -> Option<Value> {
    let system = system.map(str::trim).filter(|s| !s.is_empty())?;
    Some(json!([{
        "type": "text",
        "text": system,
        "cache_control": ephemeral(),
    }]))
}

/// Flattens history into Anthropic's message list.
///
/// Two structural differences from the normalized form. System messages do not
/// exist here — the system prompt is a top-level field, and a `Role::System`
/// inside the history would be rejected — so any that appear are folded into
/// the following user turn rather than dropped. And a tool result is a *user*
/// message containing `tool_result` blocks, not a role of its own, which is
/// already how the normalized form carries it.
pub fn messages_to_wire(request: &ChatRequest) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut carried_system = String::new();

    for message in &request.messages {
        if message.role == Role::System {
            // Nowhere to put it in the wire format, and silently discarding a
            // system message would lose an instruction somebody wrote.
            carried_system.push_str(&message.text());
            carried_system.push('\n');
            continue;
        }

        let mut blocks = block_list(message);
        if !carried_system.is_empty() && message.role == Role::User {
            blocks.insert(0, json!({ "type": "text", "text": carried_system.trim() }));
            carried_system.clear();
        }
        if blocks.is_empty() {
            continue;
        }

        out.push(json!({
            "role": match message.role {
                Role::Assistant => "assistant",
                // System was handled above; anything else is a user turn.
                _ => "user",
            },
            "content": blocks,
        }));
    }

    // A trailing system message with no user turn after it still has to arrive.
    if !carried_system.is_empty() {
        out.push(json!({
            "role": "user",
            "content": [{ "type": "text", "text": carried_system.trim() }],
        }));
    }

    mark_last_for_caching(&mut out);
    out
}

fn block_list(message: &Message) -> Vec<Value> {
    let mut blocks = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text } if text.is_empty() => {}
            ContentBlock::Text { text } => blocks.push(json!({"type": "text", "text": text})),

            // Replayed rather than dropped, unlike every other adapter here. A
            // turn that reasoned and then called a tool is only legal on the
            // next request if its thinking comes back signed and unedited.
            //
            // Without a signature it cannot be sent at all — the field is not
            // optional on the wire, and a fabricated one is worse than none —
            // so an unsigned block is omitted. That happens for reasoning the
            // provider returned redacted, and it is the known gap in this
            // adapter rather than a silent one: the request that follows may be
            // rejected, with an error that says so.
            ContentBlock::Thinking { text, signature } => {
                if let Some(signature) = signature {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": signature,
                    }));
                }
            }

            ContentBlock::ToolUse {
                id, name, input, ..
            } => blocks.push(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                // An object here, unlike OpenAI, where it is a JSON string.
                "input": input,
            })),

            // The one adapter that can carry a picture inside the result
            // itself. Everywhere else a tool's image has to be relocated into
            // the message after it; here `content` is a block list of the same
            // shape the rest of this function is building, so it goes across
            // as it stands.
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => blocks.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": tool_result_content(content),
                "is_error": is_error,
            })),

            ContentBlock::Image { mime_type, data } => blocks.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime_type,
                    "data": data,
                },
            })),
        }
    }
    blocks
}

/// Puts the second cache breakpoint on the newest turn.
///
/// Each request then reuses the whole conversation before it, and the previous
/// breakpoints stay valid read points, so an agent loop's cache hit rate grows
/// with the conversation instead of resetting on every iteration.
fn mark_last_for_caching(messages: &mut [Value]) {
    let Some(last) = messages.last_mut() else {
        return;
    };
    if let Some(block) = last
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .and_then(|blocks| blocks.last_mut())
        .and_then(Value::as_object_mut)
    {
        block.insert("cache_control".into(), ephemeral());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_and_assistant() -> ChatRequest {
        ChatRequest::new(
            "claude-opus-5",
            vec![
                Message::user("hello"),
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::tool_use(
                        "toolu_1",
                        "read_file",
                        json!({"path": "a.txt"}),
                    )],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::tool_result("toolu_1", "file body")],
                ),
            ],
        )
    }

    #[test]
    fn tool_input_stays_an_object() {
        // The structural difference from OpenAI, where the same field is a
        // JSON string assembled across frames.
        let wire = messages_to_wire(&user_and_assistant());
        assert!(wire[1]["content"][0]["input"].is_object());
        assert_eq!(wire[1]["content"][0]["input"]["path"], "a.txt");
    }

    #[test]
    fn a_tool_result_is_a_user_message_not_a_role_of_its_own() {
        let wire = messages_to_wire(&user_and_assistant());
        assert_eq!(wire[2]["role"], "user");
        assert_eq!(wire[2]["content"][0]["type"], "tool_result");
        assert_eq!(wire[2]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn the_system_prompt_leaves_the_message_list_entirely() {
        // A `system` role inside `messages` is rejected by this API, so it
        // rides the top-level field instead.
        let request = ChatRequest::new("m", vec![Message::user("hi")]).with_system("S");
        let system = system_to_wire(request.system.as_deref()).expect("a system block");
        assert_eq!(system[0]["text"], "S");
        let wire = messages_to_wire(&request);
        assert!(wire.iter().all(|m| m["role"] != "system"));
    }

    #[test]
    fn a_system_message_inside_the_history_is_folded_forward_not_dropped() {
        // Somebody wrote that instruction. Having nowhere to put it is not a
        // reason for it to vanish.
        let request = ChatRequest::new(
            "m",
            vec![
                Message::new(Role::System, vec![ContentBlock::text("be terse")]),
                Message::user("hello"),
            ],
        );
        let wire = messages_to_wire(&request);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["role"], "user");
        assert_eq!(wire[0]["content"][0]["text"], "be terse");
        assert_eq!(wire[0]["content"][1]["text"], "hello");
    }

    #[test]
    fn signed_thinking_is_replayed_and_unsigned_thinking_is_not() {
        // Signed: a turn that reasoned and then called a tool is only legal on
        // the next request with its thinking intact. Unsigned: the field is not
        // optional on the wire and this harness cannot produce one.
        let request = ChatRequest::new(
            "m",
            vec![Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking {
                        text: "reasoned".into(),
                        signature: Some("sig-abc".into()),
                    },
                    ContentBlock::thinking("redacted, no signature"),
                    ContentBlock::text("answer"),
                ],
            )],
        );
        let wire = messages_to_wire(&request);
        let blocks = wire[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2, "{blocks:#?}");
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["signature"], "sig-abc");
        assert_eq!(blocks[1]["text"], "answer");
    }

    #[test]
    fn the_system_prompt_and_the_newest_turn_are_both_cache_breakpoints() {
        // The system prompt and tool schemas are the fixed overhead `taurus
        // usage` reports; the newest turn is what makes the cached prefix grow
        // with the conversation rather than reset each iteration.
        let request = ChatRequest::new("m", vec![Message::user("hi")]).with_system("S");
        let system = system_to_wire(request.system.as_deref()).unwrap();
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");

        let wire = messages_to_wire(&request);
        assert_eq!(
            wire.last().unwrap()["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn only_the_newest_turn_carries_a_message_breakpoint() {
        // Four is the ceiling on breakpoints per request; spending one per turn
        // would exhaust it on a five-message conversation.
        let wire = messages_to_wire(&user_and_assistant());
        let marked = wire
            .iter()
            .filter(|m| {
                m["content"]
                    .as_array()
                    .is_some_and(|b| b.iter().any(|x| x.get("cache_control").is_some()))
            })
            .count();
        assert_eq!(marked, 1);
    }

    #[test]
    fn an_image_becomes_a_base64_source_block() {
        let request = ChatRequest::new(
            "m",
            vec![Message::new(
                Role::User,
                vec![
                    ContentBlock::text("what is this"),
                    ContentBlock::Image {
                        mime_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                ],
            )],
        );
        let wire = messages_to_wire(&request);
        let image = &wire[0]["content"][1];
        assert_eq!(image["source"]["type"], "base64");
        assert_eq!(image["source"]["media_type"], "image/png");
        assert_eq!(image["source"]["data"], "AAAA");
    }

    #[test]
    fn a_tool_definition_needs_no_translation() {
        let tools = tools_to_wire(&[ToolDef {
            name: "read_file".into(),
            description: "reads".into(),
            input_schema: json!({"type": "object"}),
        }]);
        assert_eq!(tools[0]["name"], "read_file");
        assert_eq!(tools[0]["input_schema"]["type"], "object");
    }
}

/// A tool's answer as Anthropic's `tool_result` content list.
///
/// Text and images map straight across. JSON becomes text, because this API has
/// no structured block inside a tool result and a bare object there is a 400 —
/// the model still reads it as JSON, which is what the tool meant by it.
fn tool_result_content(output: &taurus_provider::ToolOutput) -> Value {
    use taurus_provider::ToolResultBlock;
    let blocks: Vec<Value> = output
        .as_slice()
        .iter()
        .map(|block| match block {
            ToolResultBlock::Text { text } => json!({"type": "text", "text": text}),
            ToolResultBlock::Json { value } => {
                json!({"type": "text", "text": value.to_string()})
            }
            ToolResultBlock::Image { mime_type, data } => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime_type,
                    "data": data,
                },
            }),
        })
        .collect();
    Value::Array(blocks)
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use taurus_provider::{ChatRequest, ContentBlock, Message, Role};

    #[test]
    fn a_tool_that_returned_a_picture_sends_it_inside_the_result() {
        // The one adapter that can. Everywhere else the image has to be lifted
        // into a message of its own; here `tool_result` content is a block list
        // of the same shape this whole function builds.
        let output = taurus_provider::ToolOutput::blocks(vec![
            taurus_provider::ToolResultBlock::text("the chart"),
            taurus_provider::ToolResultBlock::image("image/png", "aGk="),
        ])
        .expect("two blocks");
        let request = ChatRequest::new(
            "claude",
            vec![Message::new(
                Role::User,
                vec![ContentBlock::tool_result("t1", output)],
            )],
        );

        let wire = messages_to_wire(&request);
        let content = &wire[0]["content"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "aGk=");
    }

    #[test]
    fn structured_output_crosses_as_text_rather_than_a_bare_object() {
        // There is no JSON block inside a `tool_result` here, and an object in
        // that position is a 400. The model still reads it as JSON.
        let request = ChatRequest::new(
            "claude",
            vec![Message::new(
                Role::User,
                vec![ContentBlock::tool_result(
                    "t1",
                    taurus_provider::ToolOutput::json(serde_json::json!({"rows": 3})),
                )],
            )],
        );
        let wire = messages_to_wire(&request);
        let content = &wire[0]["content"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].as_str().unwrap().contains("rows"));
    }
}
