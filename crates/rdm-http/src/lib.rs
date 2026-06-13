//! HTTP layer for RDM.
//!
//! Ports the Python `downloader.probe` and `providers` modules: a shared
//! reqwest client, URL probing that reports size/range-support/validators, and
//! a pluggable [`DownloadProvider`] registry that resolves a task's URL and
//! request headers before a download begins.

pub mod client;
pub mod error;
pub mod probe;
pub mod provider;

pub use client::{build_client, USER_AGENT};
pub use error::HttpError;
pub use probe::{parse_content_range, probe_url, ContentRange, ProbeResult};
pub use provider::{DownloadProvider, HttpDownloadProvider, PreparedDownload, ProviderRegistry};
