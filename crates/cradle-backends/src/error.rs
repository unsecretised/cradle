#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("no API key: set ${0} or run `cradle --setup`")]
    MissingKey(String),

    #[error("{0}")]
    Cli(String),

    #[error("provider `{0}` has no model configured; pass --model")]
    NoModel(String),

    #[error("API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("bad response from provider: {0}")]
    Protocol(String),
}

impl BackendError {
    /// Whether retrying the same request may succeed.
    pub(crate) fn retryable_status(status: u16) -> bool {
        matches!(status, 408 | 409 | 429 | 500 | 502 | 503 | 504 | 529)
    }
}
