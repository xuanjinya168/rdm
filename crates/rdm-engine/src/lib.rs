//! RDM 的分段下载引擎。
//!
//! 共享的令牌桶 [`RateLimiter`]、`.part` 文件的预留 / 发布、NTFS 稀疏文件
//! 预分配，以及驱动多连接下载的 [`DownloadEngine`]，支持
//! 动态分段切分、续传与校验和校验。

pub mod engine;
pub mod error;
pub mod files;
pub mod hls;
pub mod postprocess;
pub mod rate_limit;
pub mod sparse;

pub use engine::{DownloadEngine, EngineHandle, UpdateCallback};
pub use postprocess::{FinalizeMode, PostProcess};
pub use error::EngineError;
pub use rate_limit::RateLimiter;
