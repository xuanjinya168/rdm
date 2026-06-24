use thiserror::Error;

/// 持久化层抛出的错误。
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("下载数据库已关闭")]
    Closed,

    #[error("数据库 schema 版本高于本 RDM 版本({found} > {latest})")]
    SchemaTooNew { found: i64, latest: i64 },

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
