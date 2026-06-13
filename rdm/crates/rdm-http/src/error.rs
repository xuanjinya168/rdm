use thiserror::Error;

/// Errors raised by the HTTP layer.
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("No download provider supports URL: {0}")]
    NoProvider(String),

    #[error("Request to {url} failed with status {status}")]
    Status { status: u16, url: String },

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}
