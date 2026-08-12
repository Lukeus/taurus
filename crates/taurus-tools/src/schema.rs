//! Trimming JSON Schemas down to what a model actually reads.
//!
//! Every tool's schema goes out with every request — not once per session, but
//! once per iteration of every turn — so a hundred bytes of boilerplate per tool
//! is a hundred bytes times every request the session will ever make. It is the
//! one part of the prompt that is pure overhead: unlike history, nothing in it
//! is there because the conversation needed it.
//!
//! What comes out of `schemars` is written for validators. A model reads the
//! property names, their types, and their descriptions, and nothing else in the
//! document changes what it emits. So the rest goes.
//!
//! The rule this follows is conservative in one specific way: a key is only
//! dropped where a key means *schema keyword*. `properties` is a map of
//! user-named fields, and a tool with a `title` argument must keep it — so the
//! walk knows which positions hold schemas and which hold names, and never
//! confuses the two.

use serde_json::{Map, Value};

/// Keywords that describe the document rather than the data.
///
/// `$schema` names a spec no provider dereferences. `title` is the Rust struct
/// name, which is at best a restatement of the tool name and at worst leaks an
/// internal type into the prompt.
const DROPPED: &[&str] = &["$schema", "title"];

/// `format` values that exist to pin a Rust integer width.
///
/// Real formats — `date-time`, `uri`, `email` — tell a model something about
/// the value it should produce and are kept. These tell it that the field was a
/// `usize`, which it cannot act on and would not know what to do with.
const RUST_WIDTH_FORMATS: &[&str] = &[
    "uint", "uint8", "uint16", "uint32", "uint64", "uint128", "int8", "int16", "int32", "int64",
    "int128", "float", "double",
];

/// Positions whose value is itself a schema.
const NESTED_SCHEMA: &[&str] = &["items", "additionalProperties", "not", "if", "then", "else"];

/// Positions holding a map of *names* to schemas. The keys are user-chosen and
/// must survive; only the values are walked.
const SCHEMA_MAPS: &[&str] = &["properties", "patternProperties", "$defs", "definitions"];

/// Positions holding a list of schemas.
const SCHEMA_LISTS: &[&str] = &["oneOf", "anyOf", "allOf", "prefixItems"];

/// Returns `schema` with the parts no model reads removed.
///
/// Anything unrecognized is left exactly as it was. A schema arriving from an
/// MCP server is written by somebody else, and guessing at the meaning of a
/// keyword this does not know is how a tool call stops validating on the far
/// end.
pub fn slim(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(slim_object(map)),
        other => other.clone(),
    }
}

fn slim_object(map: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in map {
        if DROPPED.contains(&key.as_str()) {
            continue;
        }
        // `"default": null` is schemars saying a field is an `Option`, which
        // `required` already says. A real default is kept: it tells the model
        // what happens if it omits the field.
        if key == "default" && value.is_null() {
            continue;
        }
        if key == "format" && value.as_str().is_some_and(is_rust_width) {
            continue;
        }

        let slimmed = if SCHEMA_MAPS.contains(&key.as_str()) {
            match value {
                Value::Object(inner) => Value::Object(
                    inner
                        .iter()
                        .map(|(name, schema)| (name.clone(), slim(schema)))
                        .collect(),
                ),
                other => other.clone(),
            }
        } else if SCHEMA_LISTS.contains(&key.as_str()) {
            match value {
                Value::Array(items) => Value::Array(items.iter().map(slim).collect()),
                other => other.clone(),
            }
        } else if NESTED_SCHEMA.contains(&key.as_str()) {
            slim(value)
        } else {
            value.clone()
        };
        out.insert(key.clone(), slimmed);
    }
    out
}

fn is_rust_width(format: &str) -> bool {
    RUST_WIDTH_FORMATS.contains(&format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn document_keywords_go() {
        let slimmed = slim(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "ReadFileInput",
            "type": "object",
        }));
        assert_eq!(slimmed, json!({ "type": "object" }));
    }

    #[test]
    fn descriptions_and_types_stay() {
        let slimmed = slim(&json!({
            "title": "GrepInput",
            "type": "object",
            "properties": {
                "pattern": { "description": "Regular expression.", "type": "string" },
            },
            "required": ["pattern"],
        }));
        assert_eq!(
            slimmed,
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "description": "Regular expression.", "type": "string" },
                },
                "required": ["pattern"],
            })
        );
    }

    /// The one that would be a real bug: `properties` keys are the tool's own
    /// argument names, not schema keywords.
    #[test]
    fn a_property_named_title_survives() {
        let slimmed = slim(&json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "The issue title." },
                "$schema": { "type": "string" },
            },
        }));
        assert_eq!(
            slimmed["properties"]["title"],
            json!({ "type": "string", "description": "The issue title." })
        );
        assert!(slimmed["properties"].get("$schema").is_some());
    }

    #[test]
    fn a_null_default_goes_and_a_real_one_stays() {
        let slimmed = slim(&json!({
            "properties": {
                "limit": { "default": null, "type": "integer" },
                "depth": { "default": 3, "type": "integer" },
            },
        }));
        assert!(slimmed["properties"]["limit"].get("default").is_none());
        assert_eq!(slimmed["properties"]["depth"]["default"], json!(3));
    }

    #[test]
    fn a_rust_integer_width_goes_and_a_real_format_stays() {
        let slimmed = slim(&json!({
            "properties": {
                "offset": { "format": "uint", "type": "integer" },
                "due": { "format": "date-time", "type": "string" },
            },
        }));
        assert!(slimmed["properties"]["offset"].get("format").is_none());
        assert_eq!(slimmed["properties"]["due"]["format"], json!("date-time"));
    }

    #[test]
    fn nesting_is_followed_everywhere_a_schema_can_hide() {
        let slimmed = slim(&json!({
            "title": "Outer",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "title": "Item", "type": "string" },
                },
                "choice": {
                    "anyOf": [{ "title": "A", "type": "string" }, { "title": "B", "type": "null" }],
                },
            },
            "$defs": {
                "Thing": { "title": "Thing", "type": "object" },
            },
        }));
        assert!(slimmed.get("title").is_none());
        assert!(slimmed["properties"]["items"]["items"]
            .get("title")
            .is_none());
        assert!(slimmed["properties"]["choice"]["anyOf"][0]
            .get("title")
            .is_none());
        assert!(slimmed["$defs"]["Thing"].get("title").is_none());
        // The name of the definition is not a keyword and must survive, or
        // every `$ref` pointing at it breaks.
        assert!(slimmed["$defs"].get("Thing").is_some());
    }

    #[test]
    fn a_ref_is_left_alone() {
        let schema = json!({
            "properties": { "thing": { "$ref": "#/$defs/Thing" } },
            "$defs": { "Thing": { "type": "object" } },
        });
        assert_eq!(slim(&schema), schema);
    }

    #[test]
    fn an_unrecognized_keyword_is_left_alone() {
        let schema = json!({ "type": "object", "x-vendor-hint": { "title": "kept" } });
        assert_eq!(slim(&schema), schema);
    }

    #[test]
    fn a_schema_that_is_not_an_object_is_returned_unchanged() {
        assert_eq!(slim(&json!(true)), json!(true));
    }
}
