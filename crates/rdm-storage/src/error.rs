use thiserror::Error;

/// Errors raised by the persistence layer.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Download database is closed")]
    Closed,

    #[error("Database schema is newer than this RDM version ({found} > {latest})")]
    SchemaTooNew { found: i64, latest: i64 },

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
