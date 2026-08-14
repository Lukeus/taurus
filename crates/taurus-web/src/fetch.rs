//! The `fetch_url` tool.
//!
//! Retrieves one page and hands the model its text. HTML becomes Markdown
//! rather than plain text because the structure is the part worth keeping: a
//! heading tells the model where it is in a document, and a link target is
//! often the next thing it needs.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolResult};

use crate::http;

/// Default ceiling on what one page contributes to the context window. Enough
/// for a documentation page; well short of a model's whole budget.
const DEFAULT_MAX_CHARS: usize = 40_000;

/// Hard ceiling, whatever the call asks for. A tool argument must not be able to
/// spend the entire context window on one page.
const MAX_CHARS: usize = 200_000;

/// Stop reading the body at this size regardless of what survives conversion —
/// a 100 MB download that ends up truncated to 40k characters still cost the
/// download. Enforced while reading rather than from `Content-Length`, which a
/// chunked response does not have to send.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Deserialize, JsonSchema)]
pub struct FetchUrlInput {
    /// Absolute `http://` or `https://` URL of the page to read.
    pub url: String,
    /// Cap on returned characters. Defaults to 40000.
    #[serde(default)]
    pub max_chars: Option<usize>,
}

pub struct FetchUrl {
    /// When false, a URL resolving to loopback or a private network is refused.
    /// See [`crate::address`] for why this tool is guarded and the others are
    /// not.
    allow_private_hosts: bool,
}

impl FetchUrl {
    pub fn new(allow_private_hosts: bool) -> Self {
        Self {
            allow_private_hosts,
        }
    }
}

#[async_trait]
impl Tool for FetchUrl {
    fn name(&self) -> &str {
        "fetch_url"
    }

    fn description(&self) -> &str {
        "Read one web page. HTML is converted to Markdown; plain text, JSON, and Markdown come \
         back as they are. Use it to read a page `web_search` found, or a URL the user gave you. \
         Long pages are truncated — narrow the request rather than fetching the same page twice."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<FetchUrlInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Network
    }

    /// The whole URL, never an abbreviation of it. Which host a request goes to
    /// is the decision being approved, and it is the part a shortened URL hides.
    fn preview(&self, input: &serde_json::Value) -> String {
        match input.get("url").and_then(|u| u.as_str()) {
            Some(url) => format!("Fetch {url}"),
            None => "Fetch a URL".to_string(),
        }
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: FetchUrlInput = parse_input(input)?;
        let url = check_scheme(input.url.trim())?;
        let limit = input.max_chars.unwrap_or(DEFAULT_MAX_CHARS).min(MAX_CHARS);

        // Parsed here rather than left to reqwest, because the guard below
        // needs the host and refusing an unparseable URL up front is a better
        // error than whatever the transport would say about it.
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| ToolError::InvalidInput(format!("'{url}' is not a valid URL: {e}")))?;

        // Two halves of one guard. The check is what produces a readable
        // refusal; the client is what enforces it, by resolving the name once
        // and connecting to what it vetted. See [`crate::address`].
        let client = if self.allow_private_hosts {
            http::client()
        } else {
            crate::address::ensure_public(&parsed)
                .await
                .map_err(ToolError::Failed)?;
            http::guarded_client()?
        };

        let response = http::send(client.get(parsed), ctx).await?;
        let status = response.status();
        // Same-host redirects are followed, so this may not be the URL that was
        // asked for. Reporting where the text actually came from is the
        // difference between the model citing a page and citing a redirect.
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Same-host redirects were followed; anything still 3xx here crossed a
        // host boundary and was stopped. Reporting where it points, rather than
        // following it, is what keeps one approval worth one host — the model
        // can ask for the target directly, and the user approves that host on
        // its own merits.
        if status.is_redirection() {
            let target = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("another host");
            return Err(ToolError::Failed(format!(
                "{final_url} redirects to {target}, which is a different host. Fetch that URL \
                 directly if you want it; each host is approved separately."
            )));
        }
        if !status.is_success() {
            return Err(ToolError::Failed(format!("{final_url} returned {status}")));
        }
        if let Some(size) = response.content_length() {
            if size > MAX_BODY_BYTES {
                return Err(ToolError::Failed(format!(
                    "{final_url} is {} MB, too large to read",
                    size / (1024 * 1024)
                )));
            }
        }

        let kind = BodyKind::of(&content_type);
        if kind == BodyKind::Binary {
            return Err(ToolError::Failed(format!(
                "{final_url} is {content_type}, which is not text. Only pages that can be read \
                 as text are supported."
            )));
        }

        let body = http::text(response, ctx, MAX_BODY_BYTES as usize).await?;
        let text = kind.render(&body);
        Ok(format_page(&final_url, &text, limit))
    }
}

/// Rejects anything that is not an absolute web URL.
///
/// `file://` would turn a network tool into an unbounded file reader that skips
/// the workspace boundary every other tool respects, and a relative URL has no
/// meaning here at all.
fn check_scheme(url: &str) -> Result<&str, ToolError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(url);
    }
    Err(ToolError::InvalidInput(format!(
        "'{url}' is not an http:// or https:// URL"
    )))
}

/// What to do with a body, decided from its content type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BodyKind {
    Html,
    /// Text that is already readable: plain, Markdown, JSON, XML, CSV, source.
    Plain,
    Binary,
}

impl BodyKind {
    fn of(content_type: &str) -> Self {
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        match mime.as_str() {
            "text/html" | "application/xhtml+xml" => Self::Html,
            // A server that says nothing is assumed to be serving text, because
            // refusing on a missing header would fail on plenty of pages that
            // read fine.
            "" => Self::Plain,
            _ if mime.starts_with("text/") => Self::Plain,
            // `+json`, `+xml`: the suffix is the readable part of the type.
            _ if mime.starts_with("application/")
                && (mime.contains("json") || mime.contains("xml")) =>
            {
                Self::Plain
            }
            _ => Self::Binary,
        }
    }

    fn render(self, body: &str) -> String {
        match self {
            Self::Html => to_markdown(body),
            _ => body.to_string(),
        }
    }
}

/// Converts HTML to Markdown, dropping the parts of a page that are furniture.
///
/// Navigation, scripts, and styles convert into paragraphs of link text that
/// read as content and are not; stripping them is worth more to a small model
/// than the handful of real links lost with them.
///
/// `head` goes too, which costs the `<title>`. Nearly every page repeats its
/// title in an `<h1>`, so keeping it means a duplicated first line on most
/// pages to preserve it on a few — and the output already names the URL the
/// text came from.
fn to_markdown(html: &str) -> String {
    let converted = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "head", "script", "style", "nav", "header", "footer", "aside", "noscript", "form",
            "svg", "iframe",
        ])
        .build()
        .convert(html)
        // A page that will not parse is still worth something as raw text; the
        // model can read through the markup if it has to.
        .unwrap_or_else(|_| html.to_string());

    collapse_blank_lines(&converted)
}

/// Markup-heavy pages convert into runs of empty lines where the stripped
/// elements were. They carry nothing and are not free.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blanks = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.trim().to_string()
}

/// Adds the provenance line and applies the cap.
///
/// The URL is stated because the model has to be able to attribute what it just
/// read, and truncation is stated because silently returning half a page is how
/// a model concludes something is absent when it was merely cut off.
fn format_page(url: &str, text: &str, limit: usize) -> String {
    let total = text.chars().count();
    let body = if total > limit {
        let kept: String = text.chars().take(limit).collect();
        format!(
            "{}\n\n[truncated at {limit} of {total} characters; re-fetch with a larger \
             `max_chars` to read the rest]",
            kept.trim_end()
        )
    } else {
        text.trim().to_string()
    };

    if body.is_empty() {
        return format!("{url} returned no readable text.");
    }
    format!("{url}\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_url_is_refused() {
        assert!(check_scheme("/docs/index.html").is_err());
    }

    #[test]
    fn a_file_url_is_refused_so_this_is_not_a_second_file_reader() {
        let err = check_scheme("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn both_web_schemes_are_accepted() {
        assert!(check_scheme("http://a.test").is_ok());
        assert!(check_scheme("https://a.test").is_ok());
    }

    #[test]
    fn html_content_types_are_recognized_with_a_charset() {
        assert_eq!(BodyKind::of("text/html; charset=utf-8"), BodyKind::Html);
        assert_eq!(BodyKind::of("TEXT/HTML"), BodyKind::Html);
    }

    #[test]
    fn text_and_structured_types_pass_through() {
        assert_eq!(BodyKind::of("text/plain"), BodyKind::Plain);
        assert_eq!(BodyKind::of("text/markdown"), BodyKind::Plain);
        assert_eq!(BodyKind::of("application/json"), BodyKind::Plain);
        assert_eq!(BodyKind::of("application/atom+xml"), BodyKind::Plain);
    }

    #[test]
    fn a_missing_content_type_is_treated_as_text() {
        assert_eq!(BodyKind::of(""), BodyKind::Plain);
    }

    #[test]
    fn binary_types_are_rejected_rather_than_rendered_as_mojibake() {
        assert_eq!(BodyKind::of("image/png"), BodyKind::Binary);
        assert_eq!(BodyKind::of("application/pdf"), BodyKind::Binary);
        assert_eq!(BodyKind::of("application/octet-stream"), BodyKind::Binary);
    }

    #[test]
    fn headings_and_links_survive_conversion() {
        let markdown = to_markdown(
            "<html><body><h1>Title</h1><p>Some <a href=\"https://x.test\">link</a>.</p></body></html>",
        );
        assert!(markdown.contains("# Title"), "{markdown}");
        assert!(markdown.contains("https://x.test"), "{markdown}");
    }

    #[test]
    fn the_title_does_not_arrive_twice() {
        // Nearly every page states its title in <head> and again in <h1>.
        let markdown = to_markdown(
            "<html><head><title>Example Domain</title></head>\
             <body><h1>Example Domain</h1><p>Body.</p></body></html>",
        );
        assert_eq!(markdown.matches("Example Domain").count(), 1, "{markdown}");
        assert!(markdown.contains("# Example Domain"), "{markdown}");
    }

    #[test]
    fn page_furniture_is_dropped() {
        let markdown = to_markdown(
            "<body><nav><a href='/a'>Home</a></nav><script>alert(1)</script>\
             <p>Real content.</p><footer>© 2026</footer></body>",
        );
        assert!(markdown.contains("Real content."), "{markdown}");
        assert!(!markdown.contains("alert(1)"), "{markdown}");
        assert!(!markdown.contains("Home"), "{markdown}");
        assert!(!markdown.contains("2026"), "{markdown}");
    }

    #[test]
    fn runs_of_blank_lines_are_collapsed() {
        assert_eq!(collapse_blank_lines("a\n\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn the_source_url_leads_the_output() {
        let page = format_page("https://a.test/doc", "Body text.", DEFAULT_MAX_CHARS);
        assert!(
            page.starts_with("https://a.test/doc\n\nBody text."),
            "{page}"
        );
    }

    #[test]
    fn truncation_says_so_and_says_how_to_get_the_rest() {
        let page = format_page("https://a.test", &"x".repeat(100), 20);
        assert!(page.contains("truncated at 20 of 100"), "{page}");
        assert!(page.contains("max_chars"), "{page}");
    }

    #[test]
    fn a_page_that_renders_to_nothing_says_that_rather_than_returning_a_bare_url() {
        let page = format_page("https://a.test", "   \n  ", DEFAULT_MAX_CHARS);
        assert!(page.contains("no readable text"), "{page}");
    }

    #[test]
    fn an_unparseable_page_still_returns_its_text() {
        // Malformed markup should cost the formatting, not the content.
        let markdown = to_markdown("<p>unclosed <b>content");
        assert!(markdown.contains("content"), "{markdown}");
    }
}
