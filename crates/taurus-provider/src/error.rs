use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("cannot reach {provider} at {base_url}: {source}")]
    Unreachable {
        provider: String,
        base_url: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The connection opened, and then nothing arrived for `after`.
    ///
    /// Apart from [`Self::Unreachable`] because the advice is different: the
    /// backend was there, so the address and the network are not what to
    /// check.
    #[error(
        "{provider} sent nothing for {} and the request was given up. The backend \
         accepted it, so it is overloaded or stuck rather than unreachable.",
        span(*.after)
    )]
    Stalled { provider: String, after: Duration },

    #[error("{provider} returned {status}: {body}{}", wait_note(.retry_after))]
    Api {
        provider: String,
        status: u16,
        body: String,
        /// How long the backend asked to be left alone, when it said.
        ///
        /// From a `Retry-After` header, or the `RetryInfo` Gemini puts in the
        /// body. The agent loop waits for this or its own backoff, whichever
        /// is longer.
        retry_after: Option<Duration>,
    },

    /// The backend failed part-way through an answer it had already begun,
    /// and said so inside the stream rather than with a status code.
    ///
    /// Its own kind rather than an API error with a made-up status: a status
    /// of 200 would file a server-side failure as a client error, and one
    /// that is never retried.
    #[error("{provider} failed part-way through its answer: {message}")]
    Stream { provider: String, message: String },

    #[error("model '{model}' is not available on {provider}")]
    ModelNotFound { provider: String, model: String },

    #[error("could not decode {provider} stream: {source}")]
    Decode {
        provider: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("{0}")]
    Protocol(String),

    #[error("request was canceled")]
    Canceled,

    #[error("missing credentials for {provider}")]
    MissingCredentials { provider: String },
}

impl ProviderError {
    /// A stable, low-cardinality name for this kind of failure.
    ///
    /// For `error.type` on a span, where the value is meant to be something a
    /// dashboard can group by. The message goes on the log event beside it,
    /// where a unique string — a URL, a body, a model name — costs nothing;
    /// here it would make every failure its own bucket and the grouping
    /// useless.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unreachable { .. } => "unreachable",
            Self::Stalled { .. } => "stalled",
            Self::Stream { .. } => "stream",
            // The status and not the body. `api_429` and `api_500` are the two
            // somebody actually charts, and they are worth telling apart.
            Self::Api { status, .. } if *status == 429 => "api_429",
            Self::Api { status, .. } if *status >= 500 => "api_5xx",
            Self::Api { .. } => "api_4xx",
            Self::ModelNotFound { .. } => "model_not_found",
            Self::Decode { .. } => "decode",
            Self::Protocol(_) => "protocol",
            Self::Canceled => "canceled",
            Self::MissingCredentials { .. } => "missing_credentials",
        }
    }

    /// Whether retrying the identical request could plausibly succeed. Used by
    /// the agent loop to decide between a retry and surfacing the failure.
    pub fn is_transient(&self) -> bool {
        match self {
            // A stall is a request that got lost, and nothing about it says
            // the next one will be. The loop still retries only when nothing
            // reached the screen, so a stream that went quiet half-way through
            // an answer surfaces rather than starting over.
            Self::Unreachable { .. } | Self::Stalled { .. } | Self::Stream { .. } => true,
            Self::Api { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }

    /// How long the backend asked to be left alone before the next attempt.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, ProviderError>;

/// A duration as a person says it: "45 seconds", "3 minutes".
fn span(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 1.0 {
        return format!("{} ms", duration.as_millis());
    }
    let seconds = seconds.round() as u64;
    match seconds {
        1 => "1 second".into(),
        s if s < 120 => format!("{s} seconds"),
        s => format!("{} minutes", s.div_ceil(60)),
    }
}

/// The part of an API error that says how long the backend asked to wait.
fn wait_note(retry_after: &Option<Duration>) -> String {
    match retry_after {
        Some(wait) => format!(" (it asks to wait {} before trying again)", span(*wait)),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_api_error_says_how_long_the_backend_asked_to_wait() {
        let error = ProviderError::Api {
            provider: "anthropic".into(),
            status: 429,
            body: "rate_limit_error: slow down".into(),
            retry_after: Some(Duration::from_secs(20)),
        };
        assert_eq!(
            error.to_string(),
            "anthropic returned 429: rate_limit_error: slow down \
             (it asks to wait 20 seconds before trying again)"
        );
        assert_eq!(error.retry_after(), Some(Duration::from_secs(20)));
    }

    #[test]
    fn a_stall_names_how_long_it_waited() {
        let error = ProviderError::Stalled {
            provider: "openai".into(),
            after: Duration::from_secs(600),
        };
        assert!(
            error
                .to_string()
                .starts_with("openai sent nothing for 10 minutes"),
            "{error}"
        );
        assert_eq!(error.kind(), "stalled");
    }

    #[test]
    fn a_failure_inside_a_stream_is_a_server_failure_worth_retrying() {
        let error = ProviderError::Stream {
            provider: "ollama".into(),
            message: "model runner has unexpectedly stopped".into(),
        };
        assert!(error.is_transient());
        assert_eq!(error.kind(), "stream");
        assert!(error.to_string().contains("part-way"), "{error}");
    }
}
