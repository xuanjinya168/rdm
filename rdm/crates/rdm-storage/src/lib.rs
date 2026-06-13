//! SQLite persistence layer for RDM.
//!
//! Ports the Python `database` and `migrations` modules: a single connection
//! serialized behind a mutex (download workers checkpoint frequently, so the
//! per-operation connect + PRAGMA cost dominated), WAL journaling, and a
//! `PRAGMA user_version` migration scheme kept byte-compatible with existing
//! PyDM databases.

pub mod database;
pub mod error;
pub mod migrations;

pub use database::DownloadDatabase;
pub use error::StoreError;
