//! Core domain layer for RDM.
//!
//! Pure, dependency-light building blocks shared by the engine, storage and
//! UI crates: the task/segment model, input validation, segment planning and
//! application settings. Nothing here performs network or database I/O, so the
//! whole crate is unit-testable in isolation — mirroring the Python `models`,
//! `validation`, `downloader.segments` and `config` modules.

pub mod config;
pub mod error;
pub mod models;
pub mod segments;
pub mod validation;

pub use error::CoreError;
pub use models::{DownloadTask, Segment, TaskStatus};
