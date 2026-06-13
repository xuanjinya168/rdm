use thiserror::Error;

/// Errors surfaced by the download manager.
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("Download manager is shutting down")]
    ShuttingDown,

    #[error("task not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Domain(#[from] rdm_domain::CoreError),

    #[error(transparent)]
    Store(#[from] rdm_storage::StoreError),
}
