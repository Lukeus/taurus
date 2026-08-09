//! Schema-directed coercion of tool input.
//!
//! Small local models stringify scalars constantly: `"replace_all": "false"`,
//! `"timeout_secs": "30"`. Strict deserialization bounces those, the model
//! usually repeats the same mistake, and a turn burns three iterations on a
//! type mismatch that carries no ambiguity at all.
//!
//! Coercion is driven by the tool's own JSON Schema rather than guessed from
//! the value, so a genuine string field holding `"false"` is left alone. It
//! only ever converts between scalar representations of the *same* value, and
//! only where the schema names the target type.

use serde_json::{Map, Value};

/// Rewrites `input` so scalars match the types `schema` declares.
///
/// Unknown fields, unknown schema shapes, and values that cannot be converted
/// are passed through untouched; the deserializer still has the final say.
pub fn coerce(input: Value, schema: &Value) -> Value {
    coerce_inner(input, schema, schema, 0)
}

/// Bounds recursion on schemas that reference themselves through `$defs`.
const MAX_DEPTH: usize = 12;

fn coerce_inner(value: Value, schema: &Value, root: &Value, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return value;
    }
    let schema = resolve_ref(schema, root);

    // Option<T> and unions render as `anyOf`/`oneOf`; try each branch and keep
    // the first that changes something into a declared type.
    for key in ["anyOf", "oneOf"] {
        if let Some(branches) = schema.get(key).and_then(Value::as_array) {
            for branch in branches {
                let candidate = coerce_inner(value.clone(), branch, root, depth + 1);
                if candidate != value {
                    return candidate;
                }
            }
            return value;
        }
    }

    match type_names(&schema) {
        types if types.iter().any(|t| t == "object") => match value {
            Value::Object(map) => Value::Object(coerce_object(map, &schema, root, depth)),
            other => other,
        },
        types if types.iter().any(|t| t == "array") => match value {
            Value::Array(items) => {
                let item_schema = schema.get("items").cloned().unwrap_or(Value::Null);
                Value::Array(
                    items
                        .into_iter()
                        .map(|item| coerce_inner(item, &item_schema, root, depth + 1))
                        .collect(),
                )
            }
            other => other,
        },
        types => coerce_scalar(value, &types),
    }
}

fn coerce_object(
    map: Map<String, Value>,
    schema: &Value,
    root: &Value,
    depth: usize,
) -> Map<String, Value> {
    let properties = schema.get("properties").and_then(Value::as_object);
    map.into_iter()
        .map(|(key, value)| {
            let coerced = match properties.and_then(|p| p.get(&key)) {
                Some(property) => coerce_inner(value, property, root, depth + 1),
                // No declared type for this key: leave it for the deserializer
                // to accept or reject.
                None => value,
            };
            (key, coerced)
        })
        .collect()
}

fn coerce_scalar(value: Value, types: &[String]) -> Value {
    let wants = |name: &str| types.iter().any(|t| t == name);

    match &value {
        Value::String(s) => {
            let trimmed = s.trim();
            if wants("boolean") {
                match trimmed.to_ascii_lowercase().as_str() {
                    "true" | "yes" | "1" => return Value::Bool(true),
                    "false" | "no" | "0" => return Value::Bool(false),
                    _ => {}
                }
            }
            if wants("integer") {
                if let Ok(n) = trimmed.parse::<i64>() {
                    return Value::from(n);
                }
            }
            if wants("number") {
                if let Ok(n) = trimmed.parse::<f64>() {
                    if let Some(n) = serde_json::Number::from_f64(n) {
                        return Value::Number(n);
                    }
                }
            }
            value
        }
        // The mirror case: a number or bool where a string is expected.
        Value::Number(n) if wants("string") && !wants("number") && !wants("integer") => {
            Value::String(n.to_string())
        }
        Value::Bool(b) if wants("string") && !wants("boolean") => Value::String(b.to_string()),
        _ => value,
    }
}

/// The `type` field, normalized: it may be a string or an array of strings.
fn type_names(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        // A schema with only `properties` and no `type` is still an object.
        _ if schema.get("properties").is_some() => vec!["object".into()],
        _ => Vec::new(),
    }
}

/// Follows a local `#/$defs/Name` or `#/definitions/Name` reference.
fn resolve_ref(schema: &Value, root: &Value) -> Value {
    let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
        return schema.clone();
    };
    let Some(path) = reference.strip_prefix("#/") else {
        return schema.clone();
    };
    let mut cursor = root;
    for segment in path.split('/') {
        match cursor.get(segment) {
            Some(next) => cursor = next,
            None => return schema.clone(),
        }
    }
    cursor.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "replace_all": { "type": "boolean" },
                "timeout_secs": { "type": ["integer", "null"] },
                "ratio": { "type": "number" },
                "args": { "type": "array", "items": { "type": "string" } },
                "nested": {
                    "type": "object",
                    "properties": { "deep": { "type": "boolean" } }
                }
            }
        })
    }

    #[test]
    fn coerces_a_stringified_boolean() {
        // The exact failure seen from llama3.2 in a live run.
        let out = coerce(json!({"replace_all": "false"}), &schema());
        assert_eq!(out["replace_all"], json!(false));
    }

    #[test]
    fn accepts_the_usual_boolean_spellings() {
        for (input, expected) in [
            ("true", true),
            ("True", true),
            ("yes", true),
            ("1", true),
            ("false", false),
            ("FALSE", false),
            ("no", false),
            ("0", false),
        ] {
            let out = coerce(json!({ "replace_all": input }), &schema());
            assert_eq!(out["replace_all"], json!(expected), "for {input}");
        }
    }

    #[test]
    fn coerces_a_stringified_integer() {
        let out = coerce(json!({"timeout_secs": "30"}), &schema());
        assert_eq!(out["timeout_secs"], json!(30));
    }

    #[test]
    fn coerces_a_stringified_float() {
        let out = coerce(json!({"ratio": "0.5"}), &schema());
        assert_eq!(out["ratio"], json!(0.5));
    }

    #[test]
    fn leaves_a_genuine_string_alone() {
        // The reason coercion is schema-directed rather than value-guessed.
        let out = coerce(json!({"path": "false"}), &schema());
        assert_eq!(out["path"], json!("false"));
    }

    #[test]
    fn coerces_a_number_into_a_string_field() {
        let out = coerce(json!({"path": 42}), &schema());
        assert_eq!(out["path"], json!("42"));
    }

    #[test]
    fn does_not_touch_values_that_are_already_correct() {
        let input = json!({"replace_all": true, "path": "a.txt", "timeout_secs": 5});
        assert_eq!(coerce(input.clone(), &schema()), input);
    }

    #[test]
    fn leaves_unconvertible_strings_for_the_deserializer_to_reject() {
        let out = coerce(json!({"timeout_secs": "soon"}), &schema());
        assert_eq!(out["timeout_secs"], json!("soon"));
    }

    #[test]
    fn recurses_into_nested_objects() {
        let out = coerce(json!({"nested": {"deep": "true"}}), &schema());
        assert_eq!(out["nested"]["deep"], json!(true));
    }

    #[test]
    fn recurses_into_arrays() {
        let out = coerce(json!({"args": [1, "two", true]}), &schema());
        assert_eq!(out["args"], json!(["1", "two", "true"]));
    }

    #[test]
    fn passes_through_unknown_fields() {
        let out = coerce(json!({"surprise": "false"}), &schema());
        assert_eq!(out["surprise"], json!("false"));
    }

    #[test]
    fn resolves_local_refs() {
        let schema = json!({
            "type": "object",
            "properties": { "inner": { "$ref": "#/$defs/Inner" } },
            "$defs": {
                "Inner": {
                    "type": "object",
                    "properties": { "flag": { "type": "boolean" } }
                }
            }
        });
        let out = coerce(json!({"inner": {"flag": "true"}}), &schema);
        assert_eq!(out["inner"]["flag"], json!(true));
    }

    #[test]
    fn handles_option_style_any_of() {
        let schema = json!({
            "type": "object",
            "properties": {
                "maybe": { "anyOf": [{ "type": "boolean" }, { "type": "null" }] }
            }
        });
        let out = coerce(json!({"maybe": "true"}), &schema);
        assert_eq!(out["maybe"], json!(true));
    }

    #[test]
    fn survives_a_self_referential_schema() {
        let schema = json!({
            "type": "object",
            "properties": { "next": { "$ref": "#/$defs/Node" } },
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": { "next": { "$ref": "#/$defs/Node" } }
                }
            }
        });
        // Must terminate rather than recurse forever.
        let deep = json!({"next": {"next": {"next": {"next": {}}}}});
        let _ = coerce(deep, &schema);
    }

    #[test]
    fn a_non_object_input_is_returned_unchanged() {
        assert_eq!(coerce(json!("scalar"), &schema()), json!("scalar"));
    }
}
