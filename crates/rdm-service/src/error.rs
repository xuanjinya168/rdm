use thiserror::Error;

/// 下载管理器抛出的错误。
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("下载管理器正在关闭")]
    ShuttingDown,

    #[error("任务不存在: {0}")]
    NotFound(String),

    #[error(transparent)]
    Domain(#[from] rdm_domain::CoreError),

    #[error(transparent)]
    Store(#[from] rdm_storage::StoreError),
}
