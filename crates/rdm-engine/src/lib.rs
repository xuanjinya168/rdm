//! Segmented download engine for RDM.
//!
//! Async (tokio) port of the Python `downloader` package: a shared token-bucket
//! [`RateLimiter`], `.part` file reservation/publication, NTFS sparse-file
//! preallocation, and the [`DownloadEngine`] that drives a multi-connection
//! download with dynamic segment splitting, resume and checksum verification.

pub mod engine;
pub mod error;
pub mod files;
pub mod rate_limit;
pub mod sparse;

pub use engine::{DownloadEngine, EngineHandle, UpdateCallback};
pub use error::EngineError;
pub use rate_limit::RateLimiter;
