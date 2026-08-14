//! The HTTP client both web tools share, and the rules it applies to every
//! request they make.

use std::sync::OnceLock;
use std::time::Duration;

use taurus_tools::{ToolContext, ToolError};

/// Long enough for a slow search API, short enough that a hung endpoint does
/// not hold a turn open until the user gives up and cancels.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Sent so an operator reading their logs can tell what reached them. A blank
/// or browser-imitating agent string is the kind of thing that gets a
/// self-hosted instance to block you.
const USER_AGENT: &str = concat!("taurus/", env!("CARGO_PKG_VERSION"));

/// Everything both clients below share. The only thing that separates them is
/// which resolver they connect through.
fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .redirect(same_host_only())
}

/// One client for the process: it owns the connection pool, and building a new
/// one per call would throw away every kept-alive connection.
///
/// Unguarded, and deliberately so. What reaches this is a URL the user wrote in
/// a config file — a search backend, usually their own — and a private address
/// there is a self-hosted instance, not an attack. See [`crate::address`].
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        builder()
            .build()
            // Only fails if the TLS backend cannot initialize, which is not a
            // condition the rest of the harness could do anything about either.
            .unwrap_or_default()
    })
}

/// The client for a URL the model chose, resolving through
/// [`crate::address::PublicOnly`] so a name cannot answer publicly for the
/// check and privately for the connection.
///
/// A second pool rather than a second configuration of the first, because the
/// guard belongs to the client and a shared one would have to apply it to the
/// user's own search backend too.
///
/// Fails rather than falling back. The same TLS initialization that would sink
/// this sinks [`client`] as well, but *that* one degrading to a default client
/// costs a connection pool, and this one degrading to a default client costs
/// the guard — silently, on the single call it exists to protect.
pub fn guarded_client() -> Result<&'static reqwest::Client, ToolError> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            builder()
                .dns_resolver(crate::address::PublicOnly)
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| {
            ToolError::Failed(
                "the HTTP client that checks where a request goes could not be built, so this \
                 fetch was refused rather than sent unchecked."
                    .into(),
            )
        })
}

/// How many same-host hops to follow before calling it a loop.
const MAX_REDIRECTS: usize = 10;

/// Follows redirects only while the host stays the same.
///
/// Permission for a network call is granted per host, and the default policy
/// would quietly spend a grant for one host on a request to another: approve a
/// link shortener and the hop lands wherever it points, including somewhere on
/// the local network. Stopping at the boundary keeps one approval worth exactly
/// one host — the caller reports where it pointed, and reaching that host is a
/// decision the user gets to make on its own terms.
fn same_host_only() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let from = attempt.previous().last().and_then(|url| url.host_str());
        let to = attempt.url().host_str();
        match (from, to) {
            // Host comparison is ASCII-case-insensitive, so a redirect that
            // only changes capitalization is not treated as a new host.
            (Some(from), Some(to)) if from.eq_ignore_ascii_case(to) => {
                if attempt.previous().len() > MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else {
                    attempt.follow()
                }
            }
            _ => attempt.stop(),
        }
    })
}

/// Runs a request, giving up the moment the turn is canceled.
///
/// Without this a canceled turn still waits out the full timeout, because
/// dropping the future is the only way to abort an in-flight request.
pub async fn send(
    request: reqwest::RequestBuilder,
    ctx: &ToolContext,
) -> Result<reqwest::Response, ToolError> {
    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => Err(ToolError::Canceled),
        result = request.send() => result.map_err(|e| ToolError::Failed(describe(&e))),
    }
}

/// Reads a body, refusing to buffer more than `max_bytes` of it.
///
/// Read a chunk at a time rather than through `Response::text`, because a
/// declared `Content-Length` is the one thing a hostile or merely broken server
/// does not have to provide: a chunked response carries no length to check
/// ahead of time, and `text()` would buffer all of it before anyone could
/// object. The cap is what makes the limit real rather than advisory.
///
/// Bodies are decoded as UTF-8, replacing anything invalid. That gives up the
/// charset sniffing `text()` does for pages in legacy encodings — a rare page
/// rendered with replacement characters, against a bounded buffer on every one.
pub async fn text(
    mut response: reqwest::Response,
    ctx: &ToolContext,
    max_bytes: usize,
) -> Result<String, ToolError> {
    let mut body: Vec<u8> = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(ToolError::Canceled),
            chunk = response.chunk() => chunk.map_err(|e| ToolError::Failed(describe(&e)))?,
        };
        let Some(chunk) = chunk else { break };
        if body.len() + chunk.len() > max_bytes {
            return Err(ToolError::Failed(format!(
                "{} sent more than {} MB; too large to read",
                response.url(),
                max_bytes / (1024 * 1024)
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Turns a transport failure into something the model can act on.
///
/// reqwest's own `Display` is a chain of internal wrapper types; what matters
/// to a caller deciding whether to retry is which of the three things went
/// wrong — the name, the connection, or the clock.
pub fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return format!("timed out after {}s", TIMEOUT.as_secs());
    }
    if error.is_connect() {
        return format!("could not connect: {}", cause(error));
    }
    error.to_string()
}

/// The innermost reason, rather than the wrapper naming the URL.
///
/// A connect failure's `Display` says a request could not be sent and repeats
/// the URL the caller already has; what happened — the port was refused, the
/// certificate did not verify, [`crate::address::PublicOnly`] would not resolve
/// the name — is underneath it. For the address refusal in particular that
/// message is the whole answer, and dropping it would leave the one guard the
/// model needs to understand looking like an ordinary network blip.
fn cause(error: &reqwest::Error) -> String {
    let mut deepest: &dyn std::error::Error = error;
    while let Some(next) = deepest.source() {
        deepest = next;
    }
    deepest.to_string()
}

/// Strips HTML tags and unescapes the handful of entities that show up in
/// search snippets.
///
/// Brave marks query terms with `<strong>`, and a snippet that reaches the
/// model wearing markup invites it to echo the markup back. This is not an HTML
/// parser and does not need to be: the input is one sentence of text with
/// emphasis in it.
pub fn strip_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut depth = 0usize;
    while let Some(ch) = chars.next() {
        match ch {
            // Only a `<` that starts a tag name or a closing tag opens one. An
            // extract quoting `if x < y` would otherwise swallow the rest of
            // the snippet, and search results are full of prose about code.
            '<' if chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphabetic() || *next == '/' || *next == '!') =>
            {
                depth += 1
            }
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_markup_is_removed_from_a_snippet() {
        assert_eq!(
            strip_tags("the <strong>rust</strong> compiler"),
            "the rust compiler"
        );
    }

    #[test]
    fn entities_are_unescaped() {
        assert_eq!(strip_tags("a &amp; b &quot;c&quot;"), "a & b \"c\"");
    }

    #[test]
    fn whitespace_left_behind_by_stripping_is_collapsed() {
        assert_eq!(strip_tags("a\n\n  <b>b</b>   c"), "a b c");
    }

    #[test]
    fn a_closed_tag_leaves_the_text_around_it() {
        assert_eq!(strip_tags("safe <b>bold</b> text"), "safe bold text");
    }

    #[test]
    fn a_comparison_in_prose_is_not_mistaken_for_markup() {
        // Search extracts quote code constantly, and treating every `<` as a
        // tag open silently ate the rest of the snippet. A `<` followed by a
        // space cannot start a tag, which covers how comparisons are actually
        // written; `x <y` stays ambiguous with a real tag and is still lost.
        assert_eq!(
            strip_tags("loops while <b>i</b> < n, so the tail survives"),
            "loops while i < n, so the tail survives"
        );
    }

    #[test]
    fn an_unterminated_tag_costs_only_what_follows_it() {
        // Malformed markup is not worth failing a search over; the text before
        // the broken tag still tells the model something.
        assert_eq!(strip_tags("safe <b"), "safe");
    }
}
