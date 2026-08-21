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

/// Rewrites a JSON Schema into the OpenAPI subset this API accepts.
///
/// Gemini refuses the request outright on a keyword it does not know — the
/// whole request, every tool in it, with an error naming the schema rather
/// than the word. So this keeps what the API has a field for and drops the
/// rest, rather than listing what to remove: the schemas passing through are
/// not all generated here, and one written by an MCP server can use any
/// vocabulary it likes.
///
/// Two shapes are not unknown keywords but type errors against the proto, and
/// need rewriting rather than dropping: a union `type`, and the `oneOf` of
/// constants that `schemars` writes a Rust enum as.
///
/// The walk knows which positions hold schemas and which hold *names*. That
/// distinction is the whole reason this is not a flat filter: `properties` is
/// a map of the tool's own argument names, and `show_table` has an argument
/// called `title`. Dropping it as though it were the schema keyword of that
/// name left `required` pointing at an argument that no longer existed.
fn sanitize_schema(schema: &Value) -> Value {
    /// Keywords this API's `Schema` has a field for.
    ///
    /// Anything absent here is dropped, which loosens validation rather than
    /// changing what a valid call looks like — and the tool validates its own
    /// input on arrival regardless. That is the cheaper failure by a distance:
    /// the alternative is the entire tool list being rejected over one keyword
    /// nobody here wrote.
    const KEEP: &[&str] = &[
        "type",
        "format",
        "description",
        "nullable",
        "enum",
        "default",
        "example",
        "properties",
        "propertyOrdering",
        "required",
        "items",
        "anyOf",
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "pattern",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
    ];

    /// Positions holding a map of *names* to schemas. The keys are the tool's
    /// own argument names and must survive untouched; only the values are walked.
    const SCHEMA_MAPS: &[&str] = &["properties"];

    /// Positions holding one schema.
    const NESTED_SCHEMA: &[&str] = &["items"];

    /// Positions holding a list of schemas.
    const SCHEMA_LISTS: &[&str] = &["anyOf"];

    let Value::Object(map) = schema else {
        return schema.clone();
    };

    // Read before anything is dropped: the branches of a constant union carry
    // their value in a `const`, which is not a keyword this API keeps.
    if let Some(collapsed) = collapse_enum(map) {
        return collapsed;
    }

    let mut out = Map::new();
    let mut nullable = false;
    for (key, value) in map {
        if !KEEP.contains(&key.as_str()) {
            continue;
        }
        // `format` is only meaningful here for two string formats; anything
        // else — and every integer width `schemars` emits — is rejected.
        if key == "format" && !matches!(value.as_str(), Some("date-time") | Some("enum")) {
            continue;
        }
        // `type` is a single value here, not a list. JSON Schema allows a
        // union, and `schemars` uses one for every `Option<T>` field:
        // `["string", "null"]`. Sent as-is that is not an unknown keyword but
        // a type error against the proto — "cannot start list".
        if key == "type" {
            if let Some(union) = value.as_array() {
                let named: Vec<&str> = union.iter().filter_map(Value::as_str).collect();
                nullable |= named.contains(&"null");
                // The first named type wins. A `["string", "null"]` has exactly
                // one candidate; a genuine union of two real types cannot be
                // expressed here at all, and narrowing to one of them beats
                // failing the request.
                if let Some(kind) = named.iter().find(|k| **k != "null") {
                    out.insert(key.clone(), Value::String((*kind).to_string()));
                }
                continue;
            }
        }

        let rewritten = if SCHEMA_MAPS.contains(&key.as_str()) {
            match value {
                Value::Object(inner) => Value::Object(
                    inner
                        .iter()
                        .map(|(name, schema)| (name.clone(), sanitize_schema(schema)))
                        .collect(),
                ),
                other => other.clone(),
            }
        } else if SCHEMA_LISTS.contains(&key.as_str()) {
            match value {
                Value::Array(items) => Value::Array(items.iter().map(sanitize_schema).collect()),
                other => other.clone(),
            }
        } else if NESTED_SCHEMA.contains(&key.as_str()) {
            sanitize_schema(value)
        } else {
            // Data rather than schema — a `default` value, an `enum`'s list.
            value.clone()
        };
        out.insert(key.clone(), rewritten);
    }

    // What the union was saying, in the field this API has for it.
    if nullable {
        out.insert("nullable".to_string(), Value::Bool(true));
    }
    prune_required(&mut out);
    Value::Object(out)
}

/// Drops `required` entries naming a property that is not there.
///
/// A backstop rather than a rewrite. Nothing above removes a property any more,
/// but a schema can arrive already naming one it never defined — through a
/// `$ref` this cannot follow, or an `allOf` branch that held the definition —
/// and this API rejects the whole request for it rather than the one field.
fn prune_required(out: &mut Map<String, Value>) {
    let Some(Value::Array(required)) = out.get("required") else {
        return;
    };
    let defined = match out.get("properties") {
        Some(Value::Object(properties)) => properties,
        // No properties at all: nothing it names can be defined.
        _ => {
            out.remove("required");
            return;
        }
    };
    let kept: Vec<Value> = required
        .iter()
        .filter(|name| name.as_str().is_some_and(|name| defined.contains_key(name)))
        .cloned()
        .collect();
    if kept.len() == required.len() {
        return;
    }
    if kept.is_empty() {
        out.remove("required");
    } else {
        out.insert("required".to_string(), Value::Array(kept));
    }
}

/// Rewrites `schemars`'s spelling of a Rust enum into this API's `enum`.
///
/// A unit-variant enum arrives as a `oneOf` of single-`const` branches. This
/// API has neither keyword — it spells the same thing as a string with a list
/// of permitted values — so left alone the branches lose their `const` to the
/// drop list and the `oneOf` around them fails the request.
///
/// Returns `None` for anything that is not that exact shape, including a union
/// of real types, which this API cannot express at all and which is better left
/// to fail loudly than silently narrowed.
fn collapse_enum(map: &Map<String, Value>) -> Option<Value> {
    let branches = map.get("oneOf").or_else(|| map.get("anyOf"))?.as_array()?;

    let mut values = Vec::new();
    let mut variant_docs = Vec::new();
    let mut nullable = false;
    for branch in branches {
        let branch = branch.as_object()?;
        // `Option<Enum>` puts a bare null branch alongside the constants.
        if branch.get("type").and_then(Value::as_str) == Some("null") {
            nullable = true;
            continue;
        }
        let value = branch.get("const")?.as_str()?;
        if let Some(doc) = branch.get("description").and_then(Value::as_str) {
            variant_docs.push(format!("`{value}`: {doc}"));
        }
        values.push(Value::String(value.to_string()));
    }
    if values.is_empty() {
        return None;
    }

    // Everything that was alongside the union — `description`, `default` —
    // still applies, and is sanitized by the ordinary path.
    let mut siblings = map.clone();
    siblings.remove("oneOf");
    siblings.remove("anyOf");
    let Value::Object(mut out) = sanitize_schema(&Value::Object(siblings)) else {
        return None;
    };

    // What each variant means was written on the branch, and the branches are
    // about to stop existing. Dropping it would cost the model the one place
    // the difference between two values is explained.
    if !variant_docs.is_empty() {
        let mut description = out
            .get("description")
            .and_then(Value::as_str)
            .map(|d| format!("{d}\n\n"))
            .unwrap_or_default();
        description.push_str(&variant_docs.join("\n"));
        out.insert("description".to_string(), Value::String(description));
    }

    out.insert("type".to_string(), Value::String("string".to_string()));
    // The one `format` this API reads on a string besides `date-time`.
    out.insert("format".to_string(), Value::String("enum".to_string()));
    out.insert("enum".to_string(), Value::Array(values));
    if nullable {
        out.insert("nullable".to_string(), Value::Bool(true));
    }
    Some(Value::Object(out))
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

            ContentBlock::ToolUse {
                id,
                name,
                input,
                signature,
            } => {
                let mut part = json!({
                    "functionCall": {
                        // Sent alongside the name for the versions that read
                        // it. Where it is ignored the name still pairs the call
                        // with its answer, which is the only pairing this API
                        // defines.
                        "id": id,
                        "name": name,
                        "args": input,
                    }
                });
                // Rides the part rather than the call, which is where it
                // arrived. A thinking model signs the call it reasoned its way
                // to, and replaying that call unsigned is refused outright —
                // by name and by position in the history, for the whole
                // request. Only ever a value this API issued: nothing here can
                // produce one, which is the point of it.
                if let Some(signature) = signature {
                    part["thoughtSignature"] = json!(signature);
                }
                parts.push(part);
            }

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
                    vec![ContentBlock::tool_use(
                        "call_1",
                        "read_file",
                        json!({"path": "a.txt"}),
                    )],
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
                    vec![ContentBlock::tool_use("c", "run", json!({}))],
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
    fn an_optional_field_becomes_a_nullable_type_rather_than_a_union() {
        // `schemars` spells every `Option<T>` this way, so a single optional
        // argument anywhere in the tool list used to fail the whole request.
        let tools = tools_to_wire(&[ToolDef {
            name: "read_file".into(),
            description: "reads".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": ["integer", "null"]},
                    "edits": {
                        "type": ["array", "null"],
                        "items": {
                            "type": "object",
                            "properties": {"old": {"type": ["string", "null"]}}
                        }
                    }
                },
                "required": ["path"]
            }),
        }]);
        let props = &tools[0]["functionDeclarations"][0]["parameters"]["properties"];

        assert_eq!(props["offset"]["type"], "integer");
        assert_eq!(props["offset"]["nullable"], true);
        // Nested, because the union appears at every depth the schema does.
        assert_eq!(props["edits"]["type"], "array");
        assert_eq!(
            props["edits"]["items"]["properties"]["old"]["type"],
            "string"
        );
        assert_eq!(
            props["edits"]["items"]["properties"]["old"]["nullable"],
            true
        );
        // A type that was never a union is left exactly as it was.
        assert_eq!(props["path"]["type"], "string");
        assert!(props["path"].get("nullable").is_none());
    }

    #[test]
    fn an_argument_named_after_a_keyword_survives() {
        // What broke `show_table` and its three siblings: `title` is both a
        // schema keyword this API has no field for and the name of the tool's
        // first required argument. Dropping the property while `required` kept
        // naming it is rejected as "property is not defined".
        let tools = tools_to_wire(&[ToolDef {
            name: "show_table".into(),
            description: "shows".into(),
            input_schema: json!({
                "title": "ShowTableInput",
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Heading above the table."},
                    "format": {"type": "string"},
                    "const": {"type": "string"}
                },
                "required": ["title", "format", "const"]
            }),
        }]);
        let params = &tools[0]["functionDeclarations"][0]["parameters"];

        // The keyword goes.
        assert!(params.get("title").is_none());
        // The arguments of the same names stay, and so does what `required` says.
        assert_eq!(params["properties"]["title"]["type"], "string");
        assert_eq!(
            params["properties"]["title"]["description"],
            "Heading above the table."
        );
        assert_eq!(params["properties"]["format"]["type"], "string");
        assert_eq!(params["properties"]["const"]["type"], "string");
        assert_eq!(params["required"], json!(["title", "format", "const"]));
    }

    #[test]
    fn a_keyword_this_api_has_no_field_for_is_dropped_whoever_wrote_it() {
        // An allowlist rather than a fixed removal list: an MCP server's schema
        // is written by somebody else, and one keyword nobody here anticipated
        // would otherwise take down the whole tool list.
        let tools = tools_to_wire(&[ToolDef {
            name: "mcp_tool".into(),
            description: "from somebody else".into(),
            input_schema: json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "unevaluatedProperties": false,
                "x-vendor-hint": {"anything": true},
                "properties": {
                    "path": {
                        "type": "string",
                        "pattern": "^/",
                        "contentEncoding": "utf-8",
                        "deprecated": false
                    }
                },
                "required": ["path"]
            }),
        }]);
        let params = &tools[0]["functionDeclarations"][0]["parameters"];

        assert!(params.get("$schema").is_none());
        assert!(params.get("additionalProperties").is_none());
        assert!(params.get("unevaluatedProperties").is_none());
        assert!(params.get("x-vendor-hint").is_none());
        assert!(params["properties"]["path"]
            .get("contentEncoding")
            .is_none());
        assert!(params["properties"]["path"].get("deprecated").is_none());
        // A constraint it does have a field for is kept.
        assert_eq!(params["properties"]["path"]["pattern"], "^/");
        assert_eq!(params["required"], json!(["path"]));
    }

    #[test]
    fn required_cannot_outlive_the_property_it_names() {
        // A schema can arrive already naming a property it never defined —
        // behind a `$ref` this cannot follow. Loosening the constraint beats
        // the request being rejected for every tool at once.
        let tools = tools_to_wire(&[ToolDef {
            name: "mcp_tool".into(),
            description: "from somebody else".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path", "ghost"]
            }),
        }]);
        let params = &tools[0]["functionDeclarations"][0]["parameters"];
        assert_eq!(params["required"], json!(["path"]));

        // Nothing defined at all, so nothing can be required.
        let tools = tools_to_wire(&[ToolDef {
            name: "mcp_tool".into(),
            description: "from somebody else".into(),
            input_schema: json!({"type": "object", "required": ["ghost"]}),
        }]);
        let params = &tools[0]["functionDeclarations"][0]["parameters"];
        assert!(params.get("required").is_none());
    }

    #[test]
    fn a_union_of_constants_becomes_an_enum() {
        // How `schemars` writes a unit-variant Rust enum. This API has no
        // `oneOf` and no `const`, and rejects both by name.
        let tools = tools_to_wire(&[ToolDef {
            name: "update_plan".into(),
            description: "plans".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": {
                        "default": "todo",
                        "description": "Where the step is.",
                        "oneOf": [
                            {"const": "todo", "type": "string", "description": "Not started."},
                            {"const": "done", "type": "string", "description": "Finished."}
                        ]
                    }
                }
            }),
        }]);
        let state = &tools[0]["functionDeclarations"][0]["parameters"]["properties"]["state"];

        assert!(state.get("oneOf").is_none());
        assert_eq!(state["type"], "string");
        assert_eq!(state["format"], "enum");
        assert_eq!(state["enum"], json!(["todo", "done"]));
        // Siblings survive, and so does what each value meant.
        assert_eq!(state["default"], "todo");
        let description = state["description"].as_str().unwrap();
        assert!(description.starts_with("Where the step is."));
        assert!(description.contains("`todo`: Not started."));
        assert!(description.contains("`done`: Finished."));
    }

    #[test]
    fn an_optional_enum_keeps_its_null_branch_as_nullable() {
        let tools = tools_to_wire(&[ToolDef {
            name: "update_plan".into(),
            description: "plans".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": {
                        "oneOf": [
                            {"const": "todo", "type": "string"},
                            {"type": "null"}
                        ]
                    }
                }
            }),
        }]);
        let state = &tools[0]["functionDeclarations"][0]["parameters"]["properties"]["state"];

        assert_eq!(state["enum"], json!(["todo"]));
        assert_eq!(state["nullable"], true);
    }

    #[test]
    fn a_union_of_real_types_is_left_alone() {
        // Not the enum shape, and nothing here can express it. Narrowing it
        // silently would advertise a schema the tool does not accept.
        let branches = json!([{"type": "string"}, {"type": "object"}]);
        let tools = tools_to_wire(&[ToolDef {
            name: "mcp_tool".into(),
            description: "from somebody else".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"target": {"anyOf": branches}}
            }),
        }]);
        let target = &tools[0]["functionDeclarations"][0]["parameters"]["properties"]["target"];

        assert_eq!(target["anyOf"], branches);
        assert!(target.get("enum").is_none());
    }

    #[test]
    fn a_signed_call_is_replayed_with_its_signature() {
        // The whole reason the block carries one. Replaying the call without it
        // is refused by name and by position in the history.
        let request = ChatRequest::new(
            "gemini-2.5-pro",
            vec![Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "run_command".into(),
                    input: json!({"cmd": "ls"}),
                    signature: Some("sig-call".into()),
                }],
            )],
        );
        let wire = contents_to_wire(&request);
        assert_eq!(wire[0]["parts"][0]["thoughtSignature"], "sig-call");
        assert_eq!(wire[0]["parts"][0]["functionCall"]["name"], "run_command");
    }

    #[test]
    fn an_unsigned_call_carries_no_signature_field() {
        // Every other model on this API, and every transcript written before
        // the block had a slot for one. An empty key is not the same as none.
        let request = ChatRequest::new(
            "gemini-2.5-pro",
            vec![Message::new(
                Role::Assistant,
                vec![ContentBlock::tool_use("call_1", "run_command", json!({}))],
            )],
        );
        let wire = contents_to_wire(&request);
        assert!(wire[0]["parts"][0].get("thoughtSignature").is_none());
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
