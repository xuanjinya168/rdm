use thiserror::Error;

/// 终结一次下载运行的错误。
#[derive(Debug, Error)]
pub enum EngineError {
    /// 暂停或取消使运行提前结束；并非真正的失败。
    #[error("下载已中断")]
    Interrupted,

    /// 不可重试的、有具体描述的下载问题（大小不匹配、校验和错误、
    /// 服务器不再支持 Range、重试次数耗尽等）。
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
    /// 该错误是否值得重试，对应 Python 引擎对
    /// `(HTTPError, OSError, ValueError)` 的重试过滤规则。
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
