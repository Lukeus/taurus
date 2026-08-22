//! Gemini `generateContent` wire types.

use serde::{Deserialize, Serialize};
use taurus_provider::ChatRequest;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBody {
    pub contents: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
}

impl GenerationConfig {
    fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.max_output_tokens.is_none()
            && self.stop_sequences.is_empty()
    }
}

impl GenerateBody {
    pub fn from_request(request: &ChatRequest) -> Self {
        let config = GenerationConfig {
            temperature: request.temperature,
            max_output_tokens: request.max_tokens,
            stop_sequences: request.stop_sequences.clone(),
        };
        Self {
            contents: crate::convert::contents_to_wire(request),
            system_instruction: crate::convert::system_to_wire(request.system.as_deref()),
            tools: crate::convert::tools_to_wire(&request.tools),
            // Omitted entirely when nothing was set, rather than sent empty:
            // an empty object here is accepted but shows up in request logs as
            // a setting somebody chose.
            generation_config: (!config.is_empty()).then_some(config),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    /// Fully qualified, e.g. `models/gemini-2.5-pro`. The bare id is what every
    /// other part of the harness uses, so it is stripped on the way in.
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// The context window.
    #[serde(default)]
    pub input_token_limit: Option<u32>,
    /// Which methods the model serves. A model that cannot stream content is
    /// not one this harness can drive, so it is filtered out rather than
    /// offered and discovered at the first turn.
    #[serde(default)]
    pub supported_generation_methods: Vec<String>,
}

impl ModelEntry {
    pub fn id(&self) -> &str {
        self.name.strip_prefix("models/").unwrap_or(&self.name)
    }

    pub fn generates(&self) -> bool {
        // An empty list means the endpoint did not say, which is not the same
        // as saying no.
        self.supported_generation_methods.is_empty()
            || self
                .supported_generation_methods
                .iter()
                .any(|m| m == "generateContent")
    }
}

/// One streamed chunk. Every chunk is a whole response object, not a delta
/// envelope — the deltas are inside `candidates[].content.parts`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamChunk {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub usage_metadata: Option<UsageMetadata>,
    /// Present instead of `candidates` when the request itself was rejected.
    #[serde(default)]
    pub error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    #[serde(default)]
    pub content: Option<Content>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Content {
    #[serde(default)]
    pub parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default)]
    pub text: Option<String>,
    /// Marks `text` as reasoning rather than answer. Absent means answer.
    #[serde(default)]
    pub thought: Option<bool>,
    #[serde(default)]
    pub thought_signature: Option<String>,
    /// Arrives whole, unlike the fragmented tool inputs of the other two SSE
    /// backends. The adapter still emits start/delta/end so consumers cannot
    /// tell the difference.
    #[serde(default)]
    pub function_call: Option<FunctionCall>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionCall {
    /// Only some versions send one, which is why the adapter can synthesize it.
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    #[serde(default)]
    pub prompt_token_count: Option<u32>,
    #[serde(default)]
    pub candidates_token_count: Option<u32>,
    /// Reasoning tokens, billed as output and reported separately.
    #[serde(default)]
    pub thoughts_token_count: Option<u32>,
    /// Prompt tokens served from context caching, when it is in use.
    #[serde(default)]
    pub cached_content_token_count: Option<u32>,
}

impl UsageMetadata {
    pub fn output_total(&self) -> u32 {
        self.candidates_token_count
            .unwrap_or(0)
            .saturating_add(self.thoughts_token_count.unwrap_or(0))
    }
}

#[derive(Debug, Deserialize)]
pub struct ApiError {
    #[serde(default)]
    pub code: Option<u16>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_name_is_reduced_to_its_bare_id() {
        let entry: ModelEntry = serde_json::from_str(
            r#"{"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro",
                "inputTokenLimit":1048576,"supportedGenerationMethods":["generateContent"]}"#,
        )
        .unwrap();
        assert_eq!(entry.id(), "gemini-2.5-pro");
        assert_eq!(entry.input_token_limit, Some(1_048_576));
        assert!(entry.generates());
    }

    #[test]
    fn a_model_that_only_embeds_is_not_offered() {
        // Otherwise it reaches the picker and fails at the first turn.
        let entry: ModelEntry = serde_json::from_str(
            r#"{"name":"models/text-embedding-004","supportedGenerationMethods":["embedContent"]}"#,
        )
        .unwrap();
        assert!(!entry.generates());
    }

    #[test]
    fn a_listing_that_says_nothing_about_methods_is_not_read_as_a_refusal() {
        let entry: ModelEntry = serde_json::from_str(r#"{"name":"models/x"}"#).unwrap();
        assert!(entry.generates());
    }

    #[test]
    fn reasoning_tokens_count_toward_output() {
        // They are billed as output; reporting only the answer understates the
        // turn by however long the model thought.
        let usage = UsageMetadata {
            prompt_token_count: Some(100),
            candidates_token_count: Some(50),
            thoughts_token_count: Some(400),
            cached_content_token_count: None,
        };
        assert_eq!(usage.output_total(), 450);
    }

    #[test]
    fn a_thought_part_is_distinguishable_from_an_answer_part() {
        let part: Part =
            serde_json::from_str(r#"{"text":"hmm","thought":true,"thoughtSignature":"sig"}"#)
                .unwrap();
        assert_eq!(part.thought, Some(true));
        assert_eq!(part.thought_signature.as_deref(), Some("sig"));

        let answer: Part = serde_json::from_str(r#"{"text":"done"}"#).unwrap();
        assert_eq!(answer.thought, None);
    }

    #[test]
    fn generation_config_is_omitted_when_nothing_was_set() {
        let request = ChatRequest::new("g", vec![taurus_provider::Message::user("hi")]);
        let json = serde_json::to_value(GenerateBody::from_request(&request)).unwrap();
        assert!(json.get("generationConfig").is_none(), "{json}");
    }

    #[test]
    fn the_body_uses_camel_case_field_names() {
        // Everything else in this workspace is snake_case; this API is not.
        let request =
            ChatRequest::new("g", vec![taurus_provider::Message::user("hi")]).with_system("S");
        let json = serde_json::to_value(GenerateBody::from_request(&request)).unwrap();
        assert!(json.get("systemInstruction").is_some(), "{json}");
        assert!(json.get("system_instruction").is_none());
    }
}

/// A batch embedding request.
///
/// Every entry repeats the model name. That is the API's shape, not a mistake
/// here: `batchEmbedContents` is defined as a list of the single-content
/// requests, and the one in the URL does not stand in for them.
#[derive(Debug, Serialize)]
pub struct BatchEmbedBody {
    pub requests: Vec<EmbedRequest>,
}

#[derive(Debug, Serialize)]
pub struct EmbedRequest {
    /// Fully qualified — `models/text-embedding-004` — which is how this API
    /// names a model everywhere it appears in a body rather than a path.
    pub model: String,
    pub content: EmbedContent,
}

#[derive(Debug, Serialize)]
pub struct EmbedContent {
    pub parts: Vec<EmbedPart>,
}

#[derive(Debug, Serialize)]
pub struct EmbedPart {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct BatchEmbedResponse {
    #[serde(default)]
    pub embeddings: Vec<EmbeddingValues>,
}

#[derive(Debug, Deserialize)]
pub struct EmbeddingValues {
    /// No index here, unlike OpenAI: this API answers strictly in request
    /// order and gives nothing to check that against. The count is the only
    /// guard available, so it is the one that is enforced.
    #[serde(default)]
    pub values: Vec<f32>,
}
