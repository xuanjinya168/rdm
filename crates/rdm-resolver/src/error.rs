//! 媒体解析器抛出的错误。

/// 每个 [`MediaResolver`](crate::MediaResolver) 共享的失败模式。
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// 没有已注册的解析器识别该 URL。
    #[error("暂不支持解析该链接")]
    Unsupported,

    /// URL 匹配了解析器但缺少其所需的数据（例如无推文 ID）。
    #[error("无法从链接中解析出有效的内容 ID")]
    InvalidUrl,

    /// 上游 HTTP 请求在传输层失败。
    #[error("网络请求失败: {0}")]
    Http(#[from] reqwest::Error),

    /// 收到响应但无法解码为预期形状。
    #[error("解析响应失败: {0}")]
    Decode(String),

    /// 上游返回结构化错误（私密帖子、速率限制等）。
    #[error("{0}")]
    Upstream(String),

    /// 找到帖子但没有可下载的媒体。
    #[error("该链接中没有可下载的媒体")]
    NoMedia,
}
