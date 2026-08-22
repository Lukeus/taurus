//! Content blocks to OpenAI messages.

use taurus_provider::{relocated_note, ChatRequest, ContentBlock, Role, ToolDef};

pub fn tools_to_wire(tools: &[ToolDef]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect()
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Flattens block-structured history into OpenAI's message list.
///
/// Tool results become standalone `role: "tool"` messages keyed by
/// `tool_call_id`, and images ride along inside a multi-part user message.
pub fn messages_to_wire(request: &ChatRequest) -> Vec<serde_json::Value> {
    let mut out = Vec::new();

    if let Some(system) = request.system.as_ref().filter(|s| !s.trim().is_empty()) {
        out.push(serde_json::json!({ "role": "system", "content": system }));
    }

    // Which call each result answers, for the one thing the wire format does
    // not carry it for: the note on a relocated image, which has to name the
    // tool or the picture reads as something the user sent. Built up front
    // rather than per message because a result and its call are in different
    // messages by construction.
    let names_by_id: std::collections::HashMap<&str, &str> = request
        .messages
        .iter()
        .flat_map(|m| m.tool_uses())
        .map(|(id, name, _)| (id, name))
        .collect();

    for message in &request.messages {
        let mut text = String::new();
        let mut images = Vec::new();
        let mut tool_calls = Vec::new();
        let mut results = Vec::new();

        for block in &message.content {
            match block {
                ContentBlock::Text { text: t } => text.push_str(t),
                // Reasoning is not echoed back: OpenAI rejects unknown fields
                // on input and replaying it as text distorts the next turn.
                ContentBlock::Thinking { .. } => {}
                ContentBlock::Image { mime_type, data } => {
                    images.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{mime_type};base64,{data}") }
                    }));
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    tool_calls.push(serde_json::json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            // Arguments are a JSON *string* here, unlike Ollama.
                            "arguments": input.to_string(),
                        }
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // `role: "tool"` carries a string and nothing else here,
                    // so an image a tool handed back travels as the user
                    // message directly after it.
                    let (body, relocated) = content.split_relocating_images();
                    // No error flag on this message shape, so the marker has
                    // to be in the text.
                    let body = if *is_error {
                        format!("Error: {body}")
                    } else {
                        body
                    };
                    results.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": body,
                    }));
                    if !relocated.is_empty() {
                        let mut parts = vec![serde_json::json!({
                            "type": "text",
                            "text": relocated_note(names_by_id.get(tool_use_id.as_str()).copied(), relocated.len()),
                        })];
                        parts.extend(relocated.iter().map(|(mime_type, data)| {
                            serde_json::json!({
                                "type": "image_url",
                                "image_url": { "url": format!("data:{mime_type};base64,{data}") }
                            })
                        }));
                        results.push(serde_json::json!({
                            "role": "user",
                            "content": parts,
                        }));
                    }
                }
            }
        }

        if !text.is_empty() || !images.is_empty() || !tool_calls.is_empty() {
            let mut msg = serde_json::Map::new();
            msg.insert("role".into(), role_str(message.role).into());
            if images.is_empty() {
                msg.insert("content".into(), text.into());
            } else {
                let mut parts = vec![serde_json::json!({"type": "text", "text": text})];
                parts.extend(images);
                msg.insert("content".into(), parts.into());
            }
            if !tool_calls.is_empty() {
                msg.insert("tool_calls".into(), tool_calls.into());
            }
            // Results first when a message carries both. A `role: "tool"`
            // message answers the assistant call before it and nothing may come
            // between the two, so text sharing a message with tool results has
            // to follow them rather than lead.
            if results.is_empty() {
                out.push(serde_json::Value::Object(msg));
            } else {
                out.extend(std::mem::take(&mut results));
                out.push(serde_json::Value::Object(msg));
            }
        }
        out.extend(results);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_provider::Message;

    #[test]
    fn tool_arguments_are_serialized_as_a_string() {
        let req = ChatRequest::new(
            "m",
            vec![Message::new(
                Role::Assistant,
                vec![ContentBlock::tool_use(
                    "call_1",
                    "read_file",
                    serde_json::json!({"path": "a.txt"}),
                )],
            )],
        );
        let wire = messages_to_wire(&req);
        let args = wire[0]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("arguments must be a string, not an object");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(args).unwrap(),
            serde_json::json!({"path": "a.txt"})
        );
    }

    #[test]
    fn tool_results_become_tool_role_messages_keyed_by_call_id() {
        let req = ChatRequest::new(
            "m",
            vec![Message::new(
                Role::User,
                vec![ContentBlock::tool_result("call_1", "file body")],
            )],
        );
        let wire = messages_to_wire(&req);
        assert_eq!(wire[0]["role"], "tool");
        assert_eq!(wire[0]["tool_call_id"], "call_1");
        assert_eq!(wire[0]["content"], "file body");
    }

    #[test]
    fn system_leads_the_message_list() {
        let req = ChatRequest::new("m", vec![Message::user("hi")]).with_system("S");
        let wire = messages_to_wire(&req);
        assert_eq!(wire[0]["role"], "system");
        assert_eq!(wire[1]["role"], "user");
    }

    #[test]
    fn images_produce_a_multipart_content_array() {
        let req = ChatRequest::new(
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
        let wire = messages_to_wire(&req);
        let parts = wire[0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert!(parts[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn thinking_is_not_sent_back() {
        let req = ChatRequest::new(
            "m",
            vec![Message::new(
                Role::Assistant,
                vec![ContentBlock::thinking("internal")],
            )],
        );
        assert!(messages_to_wire(&req).is_empty());
    }
    #[test]
    fn text_beside_tool_results_follows_them() {
        // A `role: "tool"` message answers the assistant call before it and
        // nothing may come between the two. The plan rides the last message,
        // which is the one carrying a round of tool results, so leading with
        // the text would orphan every result behind it.
        let req = ChatRequest::new(
            "m",
            vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::tool_use(
                        "call_1",
                        "read_file",
                        serde_json::json!({}),
                    )],
                ),
                Message::new(
                    Role::User,
                    vec![
                        ContentBlock::tool_result("call_1", "contents"),
                        ContentBlock::text("# Your current plan"),
                    ],
                ),
            ],
        );
        let wire = messages_to_wire(&req);
        let roles: Vec<&str> = wire.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, vec!["assistant", "tool", "user"]);
        assert_eq!(wire[2]["content"], "# Your current plan");
    }

    #[test]
    fn a_tool_image_follows_its_result_as_its_own_user_message() {
        // `role: "tool"` carries a string here, so the picture cannot ride
        // inside it. It has to land immediately after, or the model has no way
        // to know which call it answers.
        let output = taurus_provider::ToolOutput::blocks(vec![
            taurus_provider::ToolResultBlock::text("the chart"),
            taurus_provider::ToolResultBlock::image("image/png", "aGk="),
        ])
        .expect("two blocks");
        let request = ChatRequest::new(
            "gpt",
            vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::tool_use("t1", "draw", serde_json::json!({}))],
                ),
                Message::new(Role::User, vec![ContentBlock::tool_result("t1", output)]),
            ],
        );

        let wire = messages_to_wire(&request);
        let tool_index = wire
            .iter()
            .position(|m| m["role"] == "tool")
            .expect("a tool message");
        let follower = &wire[tool_index + 1];

        assert_eq!(follower["role"], "user");
        assert!(
            follower["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("draw"),
            "the note must name the call: {follower}"
        );
        assert_eq!(follower["content"][1]["type"], "image_url");
        assert!(wire[tool_index]["content"]
            .as_str()
            .unwrap()
            .contains("[image 1"));
    }
}
