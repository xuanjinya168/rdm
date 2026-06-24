use thiserror::Error;

/// HTTP 层抛出的错误。
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("没有可处理该 URL 的下载 Provider:{0}")]
    NoProvider(String),

    #[error("请求 {url} 失败,状态码 {status}")]
    Status { status: u16, url: String },

    #[error("Provider 请求头名称无效:{0}")]
    InvalidHeaderName(String),

    #[error("Provider 请求头 {0} 的取值无效")]
    InvalidHeaderValue(String),

    #[error("Provider 不允许覆盖引擎管理的请求头:{0}")]
    ForbiddenHeader(String),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}
