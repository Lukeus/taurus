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

    #[error("{provider} returned {status}: {body}")]
    Api {
        provider: String,
        status: u16,
        body: String,
    },

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
            Self::Unreachable { .. } => true,
            Self::Api { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, ProviderError>;
