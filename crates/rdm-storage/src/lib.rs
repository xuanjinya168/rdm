//! SQLite persistence layer for RDM.
//!
//! A single connection serialized behind a mutex (download workers checkpoint
//! frequently, so the per-operation connect + PRAGMA cost would dominate),
//! WAL journaling, and a `PRAGMA user_version` migration scheme.

pub mod database;
pub mod error;
pub mod migrations;

pub use database::DownloadDatabase;
pub use error::StoreError;
