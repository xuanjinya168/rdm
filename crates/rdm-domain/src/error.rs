use thiserror::Error;

/// Errors raised while constructing or validating core domain values.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("Download URL cannot be empty")]
    EmptyUrl,

    #[error("SHA-256 must contain exactly 64 hexadecimal characters")]
    InvalidSha256,
}
