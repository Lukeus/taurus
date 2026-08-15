//! Normalized blocks to Gemini's `contents` shape.
//!
//! Three differences make this the least mechanical conversion in the
//! workspace, and each one is a place a naive port loses information.
//!
//! **The assistant is called `model`.** Cosmetic, and the only one that is.
//!
//! **Tool calls have no ids.** A `functionCall` carries a name and arguments;
//! the matching `functionResponse` carries a name and a result. Nothing pairs
//! one call with one result except position — which is fine until a turn makes
//! two calls to the same tool, at which point the wire format cannot express
//! what the normalized form knows. So ids are synthesized on the way in and
//! resolved back to names on the way out, from the history itself.
//!
//! **Schemas are an OpenAPI subset, not JSON Schema.** Keywords this API does
//! not know are rejected rather than ignored, so a tool advertised with a
//! `$schema` or an `additionalProperties` fails the whole request — and the
//! error names the schema, not the keyword.

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use taurus_provider::{ChatRequest, ContentBlock, Role, ToolDef};

/// The one `tools` entry, holding every declaration.
pub fn tools_to_wire(tools: &[ToolDef]) -> Vec<Value> {
    if tools.is_empty() {
        return Vec::new();
    }
    let declarations: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parameters": sanitize_schema(&t.input_schema),
            })
        })
        .collect();
    vec![json!({ "functionDeclarations": declarations })]
}

/// Strips JSON Schema keywords this API rejects.
///
/// Gemini accepts an OpenAPI 3 subset and refuses the request outright on a
/// keyword it does not know, with an error that names the tool rather than the
/// offending word. Since every schema here is generated from a Rust type, the
/// keywords that show up are predictable — which is what makes removing them a
/// fixed list rather than a validator.
///
/// Recursive because the keywords appear at every level: a `$schema` at the
/// root, an `additionalProperties` on each nested object.
fn sanitize_schema(schema: &Value) -> Value {
    /// Keys that are either meta-information or unsupported vocabulary. The
    /// last three are constraints this API has no field for; dropping them
    /// loosens validation rather than changing what a valid call looks like,
    /// and the tool validates its own input on arrival regardless.
    const DROP: &[&str] = &[
        "$schema",
        "$id",
        "$comment",
        "title",
        "additionalProperties",
        "definitions",
        "$defs",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "const",
    ];

    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                if DROP.contains(&key.as_str()) {
                    continue;
                }
                // `format` is only meaningful here for a couple of string
                // formats; anything else — and every integer width `schemars`
                // emits — is rejected.
                if key == "format" && !matches!(value.as_str(), Some("date-time") | Some("enum")) {
                    continue;
                }
                out.insert(key.clone(), sanitize_schema(value));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize_schema).collect()),
        other => other.clone(),
    }
}

/// The `systemInstruction` field. Its own top-level object, as on Anthropic.
pub fn system_to_wire(system: Option<&str>) -> Option<Value> {
    let system = system.map(str::trim).filter(|s| !s.is_empty())?;
    Some(json!({ "parts": [{ "text": system }] }))
}

/// Flattens history into `contents`.
pub fn contents_to_wire(request: &ChatRequest) -> Vec<Value> {
    // Built first and consulted second: a `functionResponse` has to name the
    // tool it answers, and by the time the result block is reached the call
    // that names it is several messages back.
    let names = tool_names_by_id(request);

    let mut out: Vec<Value> = Vec::new();
    let mut carried_system = String::new();

    for message in &request.messages {
        if message.role == Role::System {
            carried_system.push_str(&message.text());
            carried_system.push('\n');
            continue;
        }

        let mut parts = Vec::new();
        if !carried_system.is_empty() && message.role == Role::User {
            parts.push(json!({ "text": carried_system.trim() }));
            carried_system.clear();
        }
        parts.extend(part_list(message, &names));
        if parts.is_empty() {
            continue;
        }

        out.push(json!({
            "role": match message.role {
                Role::Assistant => "model",
                _ => "user",
            },
            "parts": parts,
        }));
    }

    if !carried_system.is_empty() {
        out.push(json!({
            "role": "user",
            "parts": [{ "text": carried_system.trim() }],
        }));
    }

    out
}

/// Every tool call in the history, by the id this harness gave it.
fn tool_names_by_id(request: &ChatRequest) -> HashMap<&str, &str> {
    request
        .messages
        .iter()
        .flat_map(|m| m.tool_uses())
        .map(|(id, name, _)| (id, name))
        .collect()
}

fn part_list(message: &taurus_provider::Message, names: &HashMap<&str, &str>) -> Vec<Value> {
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text } if text.is_empty() => {}
            ContentBlock::Text { text } => parts.push(json!({ "text": text })),

            // Replayed with its signature where one exists, because this API
            // also validates that a turn which reasoned before calling a tool
            // comes back intact. Without one there is nothing to prove the
            // block's origin, so it is left out rather than sent unsigned.
            ContentBlock::Thinking { text, signature } => {
                if let Some(signature) = signature {
                    parts.push(json!({
                        "text": text,
                        "thought": true,
                        "thoughtSignature": signature,
                    }));
                }
            }

            ContentBlock::ToolUse { id, name, input } => parts.push(json!({
                "functionCall": {
                    // Sent alongside the name for the versions that read it.
                    // Where it is ignored the name still pairs the call with
                    // its answer, which is the only pairing this API defines.
                    "id": id,
                    "name": name,
                    "args": input,
                }
            })),

            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                // The name is the pairing key, so a result whose call is not in
                // the history would be unattachable. That only happens on a
                // transcript truncated between the call and its answer, and
                // the id is a better guess than an empty string.
                let name = names
                    .get(tool_use_id.as_str())
                    .copied()
                    .unwrap_or(tool_use_id);
                parts.push(json!({
                    "functionResponse": {
                        "id": tool_use_id,
                        "name": name,
                        // Always an object: this API rejects a bare string
                        // here, and the error does not say that is why.
                        "response": if *is_error {
                            json!({ "error": content })
                        } else {
                            json!({ "output": content })
                        },
                    }
                }));
            }

            ContentBlock::Image { mime_type, data } => parts.push(json!({
                "inlineData": { "mimeType": mime_type, "data": data }
            })),
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_provider::Message;

    #[test]
    fn the_assistant_is_called_model() {
        let request = ChatRequest::new("g", vec![Message::assistant("hi")]);
        assert_eq!(contents_to_wire(&request)[0]["role"], "model");
    }

    #[test]
    fn a_tool_result_is_paired_by_the_name_of_the_call_it_answers() {
        // The wire format has no ids to pair on, so the name has to be
        // recovered from a call several messages back.
        let request = ChatRequest::new(
            "g",
            vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        input: json!({"path": "a.txt"}),
                    }],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::tool_result("call_1", "body")],
                ),
            ],
        );
        let wire = contents_to_wire(&request);
        assert_eq!(wire[0]["parts"][0]["functionCall"]["name"], "read_file");
        let response = &wire[1]["parts"][0]["functionResponse"];
        assert_eq!(response["name"], "read_file");
        assert_eq!(response["response"]["output"], "body");
    }

    #[test]
    fn a_failed_tool_answers_under_an_error_key() {
        let request = ChatRequest::new(
            "g",
            vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse {
                        id: "c".into(),
                        name: "run".into(),
                        input: json!({}),
                    }],
                ),
                Message::new(Role::User, vec![ContentBlock::tool_error("c", "boom")]),
            ],
        );
        let wire = contents_to_wire(&request);
        assert_eq!(
            wire[1]["parts"][0]["functionResponse"]["response"]["error"],
            "boom"
        );
    }

    #[test]
    fn a_tool_response_is_always_an_object() {
        // A bare string here is rejected, and the error does not say that is why.
        let request = ChatRequest::new(
            "g",
            vec![Message::new(
                Role::User,
                vec![ContentBlock::tool_result("orphan", "text")],
            )],
        );
        let wire = contents_to_wire(&request);
        assert!(wire[0]["parts"][0]["functionResponse"]["response"].is_object());
    }

    #[test]
    fn schema_keywords_this_api_rejects_are_removed_at_every_level() {
        // One unknown keyword fails the whole request, with an error naming the
        // tool rather than the word.
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "ReadFileInput",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer", "format": "uint32"},
                "nested": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"x": {"type": "string"}}
                }
            },
            "required": ["path"]
        });
        let tools = tools_to_wire(&[ToolDef {
            name: "read_file".into(),
            description: "reads".into(),
            input_schema: schema,
        }]);
        let params = &tools[0]["functionDeclarations"][0]["parameters"];

        assert!(params.get("$schema").is_none());
        assert!(params.get("title").is_none());
        assert!(params.get("additionalProperties").is_none());
        assert!(params["properties"]["nested"]
            .get("additionalProperties")
            .is_none());
        // An integer width is not a format this API knows.
        assert!(params["properties"]["offset"].get("format").is_none());
        // Everything load-bearing survives.
        assert_eq!(params["type"], "object");
        assert_eq!(params["properties"]["path"]["type"], "string");
        assert_eq!(params["required"][0], "path");
    }

    #[test]
    fn every_declaration_rides_one_tools_entry() {
        // Not one entry per tool: this API takes a list of lists, and the
        // outer list means something else.
        let tools = tools_to_wire(&[
            ToolDef {
                name: "a".into(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
            },
            ToolDef {
                name: "b".into(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
            },
        ]);
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0]["functionDeclarations"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn no_tools_means_no_tools_field_at_all() {
        // An empty declaration list is rejected rather than read as "none".
        assert!(tools_to_wire(&[]).is_empty());
    }

    #[test]
    fn the_system_prompt_is_its_own_object() {
        let request = ChatRequest::new("g", vec![Message::user("hi")]).with_system("S");
        let system = system_to_wire(request.system.as_deref()).unwrap();
        assert_eq!(system["parts"][0]["text"], "S");
        assert!(contents_to_wire(&request)
            .iter()
            .all(|c| c["role"] != "system"));
    }

    #[test]
    fn a_system_message_inside_the_history_is_folded_forward() {
        let request = ChatRequest::new(
            "g",
            vec![
                Message::new(Role::System, vec![ContentBlock::text("be terse")]),
                Message::user("hello"),
            ],
        );
        let wire = contents_to_wire(&request);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["parts"][0]["text"], "be terse");
        assert_eq!(wire[0]["parts"][1]["text"], "hello");
    }

    #[test]
    fn unsigned_reasoning_is_not_replayed() {
        let request = ChatRequest::new(
            "g",
            vec![Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::thinking("no signature"),
                    ContentBlock::text("answer"),
                ],
            )],
        );
        let parts = contents_to_wire(&request)[0]["parts"].clone();
        assert_eq!(parts.as_array().unwrap().len(), 1);
        assert_eq!(parts[0]["text"], "answer");
    }

    #[test]
    fn an_image_rides_inline_data() {
        let request = ChatRequest::new(
            "g",
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Image {
                    mime_type: "image/png".into(),
                    data: "AAAA".into(),
                }],
            )],
        );
        let wire = contents_to_wire(&request);
        assert_eq!(wire[0]["parts"][0]["inlineData"]["mimeType"], "image/png");
        assert_eq!(wire[0]["parts"][0]["inlineData"]["data"], "AAAA");
    }
}
