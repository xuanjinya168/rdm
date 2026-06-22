use thiserror::Error;

/// 在构造或校验核心领域值时抛出的错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("Download URL cannot be empty")]
    EmptyUrl,

    #[error("SHA-256 must contain exactly 64 hexadecimal characters")]
    InvalidSha256,
}
