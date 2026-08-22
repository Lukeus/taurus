//! The `web_search` tool.
//!
//! Three backends, one output shape. What reaches the model is a numbered list
//! of title, URL, and snippet, because that is the format it can act on: pick a
//! URL and hand it to `fetch_url`. Whatever else a backend returns — scores,
//! thumbnails, ranking metadata — is dropped rather than passed along, since
//! none of it changes what the model does next and all of it costs context.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolResult};

use crate::config::{Backend, BackendKind};
use crate::http;

/// More than this and the results stop being a list the model reads and start
/// being a wall of text it skims.
const MAX_RESULTS: u8 = 10;

/// Ceiling on a backend's response. Ten results of JSON is a few hundred
/// kilobytes at the outside; this is loose enough never to bite a real answer
/// and tight enough that a misconfigured URL cannot stream into memory.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Deserialize, JsonSchema)]
pub struct WebSearchInput {
    /// What to search for. Plain words work better than search-engine operators.
    pub query: String,
    /// How many results to return. Defaults to the backend's configured number.
    #[serde(default)]
    pub count: Option<u8>,
}

pub struct WebSearch {
    backend: Backend,
    description: String,
}

impl WebSearch {
    pub fn new(backend: Backend) -> Self {
        // The backend is named in the description because the model's judgment
        // about whether a result is worth trusting depends on where it came
        // from, and it has no other way to find out.
        let description = format!(
            "Search the web through {}. Returns a numbered list of title, URL, and snippet. \
             Use it for anything outside the workspace: current events, library documentation, \
             error messages you do not recognize. Snippets are short — when one looks like the \
             answer, follow it with `fetch_url` to read the page.",
            backend.describe()
        );
        Self {
            backend,
            description,
        }
    }
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<WebSearchInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Network
    }

    /// The query, in full and unquoted. The user is deciding whether to send
    /// these words to a third party, so the words are the thing to show them.
    fn preview(&self, input: &serde_json::Value) -> String {
        match input.get("query").and_then(|q| q.as_str()) {
            Some(query) => format!("Search {} for: {query}", self.backend.id),
            None => "Search the web".to_string(),
        }
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: WebSearchInput = parse_input(input)?;
        let query = input.query.trim();
        if query.is_empty() {
            return Err(ToolError::InvalidInput("query is empty".into()));
        }
        let count = input
            .count
            .unwrap_or(self.backend.max_results)
            .clamp(1, MAX_RESULTS);

        let response = http::send(self.request(query, count), ctx).await?;
        let status = response.status();
        let body = http::text(response, ctx, MAX_RESPONSE_BYTES).await?;

        if !status.is_success() {
            return Err(ToolError::Failed(self.explain_status(status, &body)));
        }

        let results = self.parse(&body)?;
        Ok(render(query, &results, count).into())
    }
}

impl Backend {
    /// How the backend is named to the model and in permission prompts.
    fn describe(&self) -> String {
        match self.kind {
            BackendKind::Brave => "Brave Search".to_string(),
            BackendKind::Tavily => "Tavily".to_string(),
            // A SearXNG instance has no brand to name, and which one it is
            // matters — a local instance and a public one are different
            // trust decisions.
            BackendKind::Searxng => format!("SearXNG at {}", self.base_url),
        }
    }
}

/// One result, reduced to the three fields that survive into the output.
#[derive(Debug)]
struct Hit {
    title: String,
    url: String,
    snippet: String,
}

impl WebSearch {
    fn request(&self, query: &str, count: u8) -> reqwest::RequestBuilder {
        let client = http::client();
        let base = &self.backend.base_url;
        match self.backend.kind {
            BackendKind::Brave => client
                .get(format!("{base}/res/v1/web/search"))
                .header("Accept", "application/json")
                .header(
                    "X-Subscription-Token",
                    self.backend.api_key.clone().unwrap_or_default(),
                )
                .query(&[("q", query), ("count", &count.to_string())]),
            BackendKind::Tavily => client
                .post(format!("{base}/search"))
                .bearer_auth(self.backend.api_key.clone().unwrap_or_default())
                .json(&serde_json::json!({
                    "query": query,
                    "max_results": count,
                })),
            BackendKind::Searxng => client
                .get(format!("{base}/search"))
                .query(&[("q", query), ("format", "json")]),
        }
    }

    /// Explains an error status in terms of what the user can change.
    ///
    /// A bare "401" leaves them checking the wrong thing; naming the variable
    /// their key comes from, or the setting a self-hosted instance needs, is
    /// the difference between a fixable error and a dead end.
    fn explain_status(&self, status: reqwest::StatusCode, body: &str) -> String {
        let detail = body.trim();
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(": {}", truncate(detail, 200))
        };
        match (status.as_u16(), self.backend.kind) {
            (401 | 403, BackendKind::Searxng) => format!(
                "{} refused the search ({status}). A SearXNG instance has to enable the `json` \
                 format in its settings.yml before it will answer API requests{detail}",
                self.backend.base_url
            ),
            (401 | 403, _) => format!(
                "{} rejected the API key ({status}){detail}",
                self.backend.describe()
            ),
            (429, _) => format!(
                "{} is rate limiting; wait before searching again ({status}){detail}",
                self.backend.describe()
            ),
            _ => format!("{} returned {status}{detail}", self.backend.describe()),
        }
    }

    fn parse(&self, body: &str) -> Result<Vec<Hit>, ToolError> {
        let json: serde_json::Value = serde_json::from_str(body).map_err(|e| {
            ToolError::Failed(format!(
                "{} returned something that is not JSON ({e})",
                self.backend.describe()
            ))
        })?;

        // Each backend nests its list somewhere different and names the snippet
        // field differently; past that they agree, which is why one shape comes
        // out the far side.
        let (list, snippet_field) = match self.backend.kind {
            BackendKind::Brave => (json.pointer("/web/results"), "description"),
            BackendKind::Tavily => (json.pointer("/results"), "content"),
            BackendKind::Searxng => (json.pointer("/results"), "content"),
        };

        let Some(list) = list.and_then(|l| l.as_array()) else {
            // An empty result set is a valid answer and must not read as a
            // parse failure; only a missing list is a real disagreement about
            // the response shape.
            return Ok(Vec::new());
        };

        Ok(list
            .iter()
            .filter_map(|item| {
                let url = item.get("url")?.as_str()?.to_string();
                let title = item
                    .get("title")
                    .and_then(|t| t.as_str())
                    .map(http::strip_tags)
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| url.clone());
                let snippet = item
                    .get(snippet_field)
                    .and_then(|s| s.as_str())
                    .map(http::strip_tags)
                    .unwrap_or_default();
                Some(Hit {
                    title,
                    url,
                    snippet,
                })
            })
            .collect())
    }
}

/// Caps a snippet. Tavily returns page extracts rather than one-liners, and an
/// unbounded ten of those is most of a small model's context window.
const MAX_SNIPPET: usize = 500;

fn render(query: &str, hits: &[Hit], requested: u8) -> String {
    if hits.is_empty() {
        return format!("No results for \"{query}\".");
    }

    let mut out = String::new();
    for (i, hit) in hits.iter().take(requested as usize).enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n", i + 1, hit.title, hit.url));
        if !hit.snippet.is_empty() {
            out.push_str(&format!("   {}\n", truncate(&hit.snippet, MAX_SNIPPET)));
        }
        out.push('\n');
    }
    out.push_str("Snippets are excerpts. Use `fetch_url` to read any of these pages in full.");
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BackendKind;

    fn backend(kind: BackendKind) -> Backend {
        Backend {
            id: "test".into(),
            kind,
            base_url: "http://example.invalid".into(),
            api_key: Some("key".into()),
            max_results: 5,
            allow_private_hosts: false,
        }
    }

    fn tool(kind: BackendKind) -> WebSearch {
        WebSearch::new(backend(kind))
    }

    #[test]
    fn brave_results_are_read_from_the_web_block() {
        let hits = tool(BackendKind::Brave)
            .parse(
                r#"{"web": {"results": [
                     {"title": "Rust <strong>book</strong>", "url": "https://doc.rust-lang.org/book/",
                      "description": "The <strong>book</strong>."}]}}"#,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust book");
        assert_eq!(hits[0].snippet, "The book.");
    }

    #[test]
    fn tavily_results_use_the_content_field() {
        let hits = tool(BackendKind::Tavily)
            .parse(r#"{"results": [{"title": "T", "url": "https://x.test", "content": "body"}]}"#)
            .unwrap();
        assert_eq!(hits[0].snippet, "body");
    }

    #[test]
    fn searxng_results_use_the_content_field() {
        let hits = tool(BackendKind::Searxng)
            .parse(r#"{"results": [{"title": "S", "url": "https://y.test", "content": "body"}]}"#)
            .unwrap();
        assert_eq!(hits[0].url, "https://y.test");
    }

    #[test]
    fn no_results_is_an_answer_rather_than_a_parse_failure() {
        let hits = tool(BackendKind::Brave)
            .parse(r#"{"query": {"original": "x"}}"#)
            .unwrap();
        assert!(hits.is_empty());
        assert!(render("x", &hits, 5).starts_with("No results"));
    }

    #[test]
    fn a_result_without_a_url_is_dropped_rather_than_rendered_unfollowable() {
        let hits = tool(BackendKind::Tavily)
            .parse(r#"{"results": [{"title": "no link"}, {"url": "https://ok.test"}]}"#)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://ok.test");
    }

    #[test]
    fn a_result_with_no_title_falls_back_to_its_url() {
        let hits = tool(BackendKind::Tavily)
            .parse(r#"{"results": [{"url": "https://ok.test", "content": "c"}]}"#)
            .unwrap();
        assert_eq!(hits[0].title, "https://ok.test");
    }

    #[test]
    fn a_non_json_body_says_which_backend_misbehaved() {
        let err = tool(BackendKind::Brave)
            .parse("<html>maintenance</html>")
            .unwrap_err();
        assert!(err.to_string().contains("Brave"), "{err}");
    }

    #[test]
    fn the_rendered_list_is_numbered_and_carries_urls() {
        let hits = vec![
            Hit {
                title: "First".into(),
                url: "https://a.test".into(),
                snippet: "one".into(),
            },
            Hit {
                title: "Second".into(),
                url: "https://b.test".into(),
                snippet: String::new(),
            },
        ];
        let out = render("q", &hits, 5);
        assert!(out.contains("1. First\n   https://a.test\n   one"));
        assert!(out.contains("2. Second\n   https://b.test"));
        assert!(out.contains("fetch_url"), "the next step has to be named");
    }

    #[test]
    fn a_long_snippet_is_capped() {
        let hits = vec![Hit {
            title: "T".into(),
            url: "https://a.test".into(),
            snippet: "x".repeat(MAX_SNIPPET * 2),
        }];
        assert!(render("q", &hits, 5).len() < MAX_SNIPPET * 2);
    }

    #[test]
    fn a_searxng_403_explains_the_setting_that_causes_it() {
        let message = tool(BackendKind::Searxng).explain_status(reqwest::StatusCode::FORBIDDEN, "");
        assert!(message.contains("settings.yml"), "{message}");
    }

    #[test]
    fn a_hosted_401_points_at_the_key_rather_than_the_instance() {
        let message =
            tool(BackendKind::Brave).explain_status(reqwest::StatusCode::UNAUTHORIZED, "");
        assert!(message.contains("API key"), "{message}");
    }

    #[test]
    fn a_rate_limit_says_to_wait() {
        let message =
            tool(BackendKind::Tavily).explain_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "");
        assert!(message.contains("rate limiting"), "{message}");
    }

    #[tokio::test]
    async fn an_empty_query_is_refused_before_a_request_goes_out() {
        let (ctx, _dir) = crate::test_support::allowing_ctx();
        let err = tool(BackendKind::Brave)
            .execute(serde_json::json!({"query": "   "}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn the_preview_shows_the_words_that_would_leave_the_machine() {
        let preview =
            tool(BackendKind::Brave).preview(&serde_json::json!({"query": "acme q3 leak"}));
        assert!(preview.contains("acme q3 leak"), "{preview}");
    }

    #[test]
    fn the_description_names_the_backend_so_the_model_can_weigh_the_source() {
        assert!(tool(BackendKind::Brave).description().contains("Brave"));
        assert!(tool(BackendKind::Searxng)
            .description()
            .contains("example.invalid"));
    }
}
