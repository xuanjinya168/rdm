//! RDM 的 HTTP 层。
//!
//! 移植自 Python 的 `downloader.probe` 和 `providers` 模块：
//! 共享的 reqwest 客户端、上报大小 / Range 支持 / 校验信息的 URL 探测，
//! 以及一个可插拔的 [`DownloadProvider`] 注册表，在下载开始前
//! 解析任务的 URL 与请求头。

pub mod client;
pub mod error;
pub mod probe;
pub mod provider;

pub use client::{build_client, ProxyConfig, USER_AGENT};
pub use error::HttpError;
pub use probe::{parse_content_range, probe_url, ContentRange, ProbeResult};
pub use provider::{DownloadProvider, HttpDownloadProvider, PreparedDownload, ProviderRegistry};
