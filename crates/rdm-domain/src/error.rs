use thiserror::Error;

/// 在构造或校验核心领域值时抛出的错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("下载地址不能为空")]
    EmptyUrl,

    #[error("SHA-256 必须恰好包含 64 个十六进制字符")]
    InvalidSha256,
}
