//! RDM 的下载调度层。
//!
//! [`DownloadManager`] 拥有任务表，按配置的并发上限调度排队的
//! 下载任务，并提供 UI 所需的生命周期操作（add/start/pause/
//! cancel/delete）。后台调度任务使用 [`tokio::sync::Notify`] 唤醒。

mod error;
mod manager;

pub use error::ServiceError;
pub use manager::{DownloadManager, ManagerListener};
