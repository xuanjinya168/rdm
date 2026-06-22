//! RDM 的分段下载引擎。
//!
//! Python `downloader` 包的异步（tokio）移植：共享的令牌桶
//! [`RateLimiter`]、`.part` 文件的预留 / 发布、NTFS 稀疏文件
//! 预分配，以及驱动多连接下载的 [`DownloadEngine`]，支持
//! 动态分段切分、续传与校验和校验。

pub mod engine;
pub mod error;
pub mod files;
pub mod rate_limit;
pub mod sparse;

pub use engine::{DownloadEngine, EngineHandle, UpdateCallback};
pub use error::EngineError;
pub use rate_limit::RateLimiter;
