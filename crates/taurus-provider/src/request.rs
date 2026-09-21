//! The request shape handed to every provider adapter.

use serde::{Deserialize, Serialize};

use crate::message::Message;

/// A tool as advertised to the model. `input_schema` is a JSON Schema object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    /// Kept out of `messages` so adapters can place it wherever their wire
    /// format wants it, and so the prompted-tool fallback can append to it.
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stop_sequences: Vec<String>,
    /// A JSON Schema the whole answer must satisfy, where the backend can
    /// enforce one.
    ///
    /// Constrained decoding, not a request in the prompt: a backend that
    /// supports this rejects any token that would make the output violate the
    /// schema, so malformed output stops being a thing that has to be
    /// recovered from. That is worth most on exactly the models that most
    /// often produce it.
    ///
    /// It constrains the *response*, so it belongs only to a request whose
    /// whole answer has a known shape. It cannot be used to constrain one tool
    /// call inside a turn that might equally have answered in prose — that
    /// needs the tool schemas, which are a different mechanism.
    ///
    /// Callers must not depend on it. Backends that cannot enforce a schema
    /// ignore this and answer as they would have, so whatever reads the answer
    /// parses defensively and has something to fall back to.
    pub response_schema: Option<serde_json::Value>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            ..Default::default()
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_response_schema(mut self, schema: serde_json::Value) -> Self {
        self.response_schema = Some(schema);
        self
    }
}
