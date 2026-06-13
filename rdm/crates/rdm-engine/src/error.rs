use thiserror::Error;

/// Errors that can end a download run.
#[derive(Debug, Error)]
pub enum EngineError {
    /// A pause or cancel unwound the run; not a real failure.
    #[error("download interrupted")]
    Interrupted,

    /// A non-retryable, descriptive download problem (size mismatch, bad
    /// checksum, a server that stopped honoring ranges, exhausted retries).
    #[error("{0}")]
    Download(String),

    #[error(transparent)]
    Http(#[from] rdm_http::HttpError),

    #[error(transparent)]
    Store(#[from] rdm_storage::StoreError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}

impl EngineError {
    /// Whether a transfer error of this kind is worth retrying, mirroring the
    /// Python engine's `(HTTPError, OSError, ValueError)` retry filter.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            EngineError::Http(_)
                | EngineError::Io(_)
                | EngineError::Reqwest(_)
                | EngineError::Download(_)
        )
    }
}
