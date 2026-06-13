//! Errors surfaced by media resolvers.

/// Failure modes shared by every [`MediaResolver`](crate::MediaResolver).
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// No registered resolver recognised the URL.
    #[error("暂不支持解析该链接")]
    Unsupported,

    /// The URL matched a resolver but lacked the data it needs (e.g. no tweet id).
    #[error("无法从链接中解析出有效的内容 ID")]
    InvalidUrl,

    /// The upstream HTTP request failed at the transport level.
    #[error("网络请求失败: {0}")]
    Http(#[from] reqwest::Error),

    /// The response was received but could not be decoded into the expected shape.
    #[error("解析响应失败: {0}")]
    Decode(String),

    /// The upstream returned a structured error (private post, rate limit, …).
    #[error("{0}")]
    Upstream(String),

    /// The post was found but carried no downloadable media.
    #[error("该链接中没有可下载的媒体")]
    NoMedia,
}
