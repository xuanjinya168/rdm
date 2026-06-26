//! RDM 的核心领域层。
//!
//! 一组无副作用、依赖极少的构件，供 engine、storage 与 UI crate 共用：
//! 任务 / 分段模型、输入校验、分段规划以及应用设置。本层不进行任何
//! 网络或数据库 I/O，因此整个 crate 可以独立进行单元测试 ——
//! 对应于 `models`、`validation`、`segments` 与 `config` 模块。

pub mod config;
pub mod error;
pub mod models;
pub mod segments;
pub mod validation;

pub use error::CoreError;
pub use models::{DownloadTask, Segment, TaskStatus};
