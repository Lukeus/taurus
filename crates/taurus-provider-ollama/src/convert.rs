//! Translation between normalized content blocks and Ollama's message shape.

use std::collections::HashMap;

use taurus_provider::{ChatRequest, ContentBlock, Role, ToolDef};

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
                    let body = if *is_error {
                        format!("Error: {content}")
                    } else {
                        content.clone()
                    };
                    results.push(WireMessage {
                        role: "tool",
                        content: body,
                        images: Vec::new(),
                        tool_calls: Vec::new(),
                        tool_name: name.map(str::to_string),
                    });
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
