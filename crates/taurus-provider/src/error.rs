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
