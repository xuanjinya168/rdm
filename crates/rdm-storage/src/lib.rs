//! RDM 的 SQLite 持久化层。
//!
//! 由互斥锁保护的单一连接（下载工作线程频繁写入检查点，
//! 因此每次操作都重新连接并设置 PRAGMA 会成为主要开销）、
//! WAL 日志模式以及基于 `PRAGMA user_version` 的迁移机制。

pub mod database;
pub mod error;
pub mod migrations;

pub use database::DownloadDatabase;
pub use error::StoreError;
