//! Translation between normalized content blocks and Ollama's message shape.

use std::collections::HashMap;

use taurus_provider::{relocated_note, ChatRequest, ContentBlock, Role, ToolDef};

use crate::wire::{WireFunction, WireMessage, WireTool, WireToolCall, WireToolCallFunction};

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

pub fn tools_to_wire(tools: &[ToolDef]) -> Vec<WireTool> {
    tools
        .iter()
        .map(|t| WireTool {
            kind: "function",
            function: WireFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect()
}

/// Flattens the block-structured history into Ollama's message list.
///
/// Two structural mismatches are handled here. Ollama has no tool-result
/// content block, so each result becomes its own `role: "tool"` message; and
/// it identifies results by tool *name* rather than by call id, so we carry a
/// map built from the assistant turns we have already walked.
pub fn messages_to_wire(request: &ChatRequest) -> Vec<WireMessage> {
    let mut out = Vec::new();

    if let Some(system) = request.system.as_ref().filter(|s| !s.trim().is_empty()) {
        out.push(WireMessage {
            role: "system",
            content: system.clone(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_name: None,
        });
    }

    let mut names_by_id: HashMap<&str, &str> = HashMap::new();

    for message in &request.messages {
        let mut text = String::new();
        let mut images = Vec::new();
        let mut tool_calls = Vec::new();
        let mut results = Vec::new();

        for block in &message.content {
            match block {
                ContentBlock::Text { text: t } => text.push_str(t),
                // Prior-turn reasoning is not resent: it inflates the prompt and
                // Ollama does not accept it as input.
                ContentBlock::Thinking { .. } => {}
                ContentBlock::Image { data, .. } => images.push(data.clone()),
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    names_by_id.insert(id.as_str(), name.as_str());
                    tool_calls.push(WireToolCall {
                        id: Some(id.clone()),
                        function: WireToolCallFunction {
                            name: name.clone(),
                            arguments: input.clone(),
                        },
                    });
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let name = names_by_id.get(tool_use_id.as_str()).copied();
                    // A tool message here is text and an `images` array it does
                    // not read, so a picture a tool handed back travels as the
                    // user message straight after it.
                    let (body, relocated) = content.split_relocating_images();
                    // No error flag on this message shape, so the marker has
                    // to be in the text.
                    let body = if *is_error {
                        format!("Error: {body}")
                    } else {
                        body
                    };
                    results.push(WireMessage {
                        role: "tool",
                        content: body,
                        images: Vec::new(),
                        tool_calls: Vec::new(),
                        tool_name: name.map(str::to_string),
                    });
                    if !relocated.is_empty() {
                        results.push(WireMessage {
                            role: "user",
                            content: relocated_note(name, relocated.len()),
                            images: relocated
                                .iter()
                                .map(|(_, data)| (*data).to_string())
                                .collect(),
                            tool_calls: Vec::new(),
                            tool_name: None,
                        });
                    }
                }
            }
        }

        if !text.is_empty() || !images.is_empty() || !tool_calls.is_empty() {
            let msg = WireMessage {
                role: role_str(message.role),
                content: text,
                images,
                tool_calls,
                tool_name: None,
            };
            // Results first when a message carries both: a `role: "tool"`
            // message answers the call before it, and text sharing a message
            // with tool results has to follow them rather than lead.
            out.extend(std::mem::take(&mut results));
            out.push(msg);
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
    fn system_is_prepended_as_a_message() {
        let req = ChatRequest::new("m", vec![Message::user("hi")]).with_system("S");
        let wire = messages_to_wire(&req);
        assert_eq!(wire[0].role, "system");
        assert_eq!(wire[0].content, "S");
        assert_eq!(wire[1].role, "user");
    }

    #[test]
    fn blank_system_is_omitted() {
        let req = ChatRequest::new("m", vec![Message::user("hi")]).with_system("   ");
        assert_eq!(messages_to_wire(&req)[0].role, "user");
    }

    #[test]
    fn tool_result_becomes_a_tool_message_named_after_its_call() {
        let req = ChatRequest::new(
            "m",
            vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::tool_use(
                        "t1",
                        "read_file",
                        serde_json::json!({"path": "a"}),
                    )],
                ),
                Message::new(Role::User, vec![ContentBlock::tool_result("t1", "body")]),
            ],
        );
        let wire = messages_to_wire(&req);
        assert_eq!(wire[0].role, "assistant");
        assert_eq!(wire[0].tool_calls[0].function.name, "read_file");
        assert_eq!(wire[1].role, "tool");
        assert_eq!(wire[1].tool_name.as_deref(), Some("read_file"));
        assert_eq!(wire[1].content, "body");
    }

    #[test]
    fn error_results_are_labeled_for_the_model() {
        let req = ChatRequest::new(
            "m",
            vec![Message::new(
                Role::User,
                vec![ContentBlock::tool_error("t1", "no such file")],
            )],
        );
        let wire = messages_to_wire(&req);
        assert_eq!(wire[0].content, "Error: no such file");
    }

    #[test]
    fn thinking_only_turns_do_not_emit_empty_messages() {
        let req = ChatRequest::new(
            "m",
            vec![
                Message::user("hi"),
                Message::new(Role::Assistant, vec![ContentBlock::thinking("hmm")]),
            ],
        );
        let wire = messages_to_wire(&req);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].role, "user");
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
        let roles: Vec<&str> = wire.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec!["assistant", "tool", "user"]);
        assert_eq!(wire[2].content, "# Your current plan");
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use taurus_provider::{ChatRequest, Message};

    fn with_image() -> taurus_provider::ToolOutput {
        taurus_provider::ToolOutput::blocks(vec![
            taurus_provider::ToolResultBlock::text("the chart"),
            taurus_provider::ToolResultBlock::image("image/png", "aGk="),
        ])
        .expect("two blocks")
    }

    #[test]
    fn a_tool_image_follows_its_result_as_its_own_user_message() {
        // A tool message here has an `images` array the server does not read
        // for that role, so the picture travels as the message straight after.
        let request = ChatRequest::new(
            "llama",
            vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::tool_use("t1", "draw", serde_json::json!({}))],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::tool_result("t1", with_image())],
                ),
            ],
        );

        let wire = messages_to_wire(&request);
        let tool_index = wire
            .iter()
            .position(|m| m.role == "tool")
            .expect("a tool message");
        let follower = &wire[tool_index + 1];

        assert_eq!(follower.role, "user");
        assert_eq!(follower.images, vec!["aGk=".to_string()]);
        assert!(
            follower.content.contains("draw"),
            "the note must name the call: {}",
            follower.content
        );
        assert!(
            wire[tool_index].content.contains("[image 1"),
            "the result must say where the picture went: {}",
            wire[tool_index].content
        );
    }

    #[test]
    fn a_text_only_result_gains_no_extra_message() {
        // The overwhelmingly common case has to stay byte-identical.
        let request = ChatRequest::new(
            "llama",
            vec![Message::new(
                Role::User,
                vec![ContentBlock::tool_result("t1", "459 lines")],
            )],
        );
        let wire = messages_to_wire(&request);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].content, "459 lines");
    }
}
