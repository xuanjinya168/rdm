use thiserror::Error;

/// HTTP 层抛出的错误。
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("No download provider supports URL: {0}")]
    NoProvider(String),

    #[error("Request to {url} failed with status {status}")]
    Status { status: u16, url: String },

    #[error("Invalid provider header name: {0}")]
    InvalidHeaderName(String),

    #[error("Invalid value for provider header {0}")]
    InvalidHeaderValue(String),

    #[error("Provider may not override engine-managed header: {0}")]
    ForbiddenHeader(String),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}
