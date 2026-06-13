//! Download orchestration for RDM. Async port of the Python `manager` module.
//!
//! [`DownloadManager`] owns the task table, schedules queued downloads up to the
//! configured concurrency, and exposes the lifecycle controls (add/start/pause/
//! cancel/delete) the UI drives. A background scheduler task replaces the
//! Python scheduler thread, woken by a [`tokio::sync::Notify`] instead of a
//! condition variable.

mod error;
mod manager;

pub use error::ServiceError;
pub use manager::{DownloadManager, ManagerListener};
