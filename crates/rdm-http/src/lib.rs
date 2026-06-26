//! RDM 的 HTTP 层。
//!
//! 共享的 reqwest 客户端、URL 探测，以及可插拔的 [`DownloadProvider`] 注册表。

pub mod client;
pub mod error;
pub mod probe;
pub mod provider;

pub use client::{build_client, ProxyConfig, USER_AGENT};
pub use error::HttpError;
pub use probe::{parse_content_range, probe_url, ContentRange, ProbeResult};
pub use provider::{DownloadProvider, HttpDownloadProvider, PreparedDownload, ProviderRegistry};
