//! 下载 Provider。从 Python 的 `providers` 包迁移而来。
//!
//! Provider 将任务中存储的 URL 转化为一次引擎执行实际使用的
//! URL 与请求头 —— 是鉴权或签名源的扩展点。普通的 HTTP/HTTPS
//! 由内置的 [`HttpDownloadProvider`] 处理。

use rdm_domain::DownloadTask;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT_ENCODING, IF_RANGE, RANGE};
use url::Url;

use crate::error::HttpError;

/// 一次下载运行中由 Provider 解析出的 URL 与请求头。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedDownload {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl PreparedDownload {
    /// 为 `url` 构造一个没有额外请求头的 PreparedDownload。
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    /// 校验 Provider 请求头并转换为 reqwest 所需的表示。
    /// Range 与编码相关的请求头仍由引擎统一管理。
    pub fn request_headers(&self) -> Result<HeaderMap, HttpError> {
        let mut headers = HeaderMap::new();
        for (raw_name, raw_value) in &self.headers {
            let name = HeaderName::from_bytes(raw_name.as_bytes())
                .map_err(|_| HttpError::InvalidHeaderName(raw_name.clone()))?;
            if name == RANGE || name == IF_RANGE || name == ACCEPT_ENCODING {
                return Err(HttpError::ForbiddenHeader(raw_name.clone()));
            }
            let value = HeaderValue::from_str(raw_value)
                .map_err(|_| HttpError::InvalidHeaderValue(raw_name.clone()))?;
            headers.append(name, value);
        }
        Ok(headers)
    }
}

/// 将一个任务解析为 [`PreparedDownload`]。实现必须可跨工作线程共享。
pub trait DownloadProvider: Send + Sync {
    fn name(&self) -> &str;
    fn can_handle(&self, url: &str) -> bool;
    fn prepare(&self, task: &DownloadTask) -> Result<PreparedDownload, HttpError>;
}

/// 内置的 Provider，用于普通的 `http`/`https` URL。
pub struct HttpDownloadProvider;

impl DownloadProvider for HttpDownloadProvider {
    fn name(&self) -> &str {
        "http"
    }

    fn can_handle(&self, url: &str) -> bool {
        Url::parse(url)
            .map(|parsed| matches!(parsed.scheme(), "http" | "https"))
            .unwrap_or(false)
    }

    fn prepare(&self, task: &DownloadTask) -> Result<PreparedDownload, HttpError> {
        Ok(PreparedDownload::new(task.url.clone()))
    }
}

/// 有序的 Provider 列表；首个能处理该 URL 的 Provider 胜出。
pub struct ProviderRegistry {
    providers: Vec<Box<dyn DownloadProvider>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self {
            providers: vec![Box::new(HttpDownloadProvider)],
        }
    }
}

impl ProviderRegistry {
    /// 使用显式 Provider 列表构造注册表。
    pub fn new(providers: Vec<Box<dyn DownloadProvider>>) -> Self {
        Self { providers }
    }

    /// 注册一个 Provider。当 `first` 为 true 时插入到列表前端（拥有更高优先级）。
    pub fn register(&mut self, provider: Box<dyn DownloadProvider>, first: bool) {
        if first {
            self.providers.insert(0, provider);
        } else {
            self.providers.push(provider);
        }
    }

    /// 使用首个匹配的 Provider 解析 `task`。
    pub fn prepare(&self, task: &DownloadTask) -> Result<PreparedDownload, HttpError> {
        for provider in &self.providers {
            if provider.can_handle(&task.url) {
                return provider.prepare(task);
            }
        }
        Err(HttpError::NoProvider(task.url.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(url: &str) -> DownloadTask {
        DownloadTask::create(url, "C:/dl", 8, "", "").unwrap()
    }

    #[test]
    fn http_provider_handles_http_and_https() {
        let provider = HttpDownloadProvider;
        assert!(provider.can_handle("https://example.test/file"));
        assert!(provider.can_handle("http://example.test/file"));
        assert!(!provider.can_handle("ftp://example.test/file"));
        assert!(!provider.can_handle("not a url"));
    }

    #[test]
    fn registry_prepares_supported_urls() {
        let registry = ProviderRegistry::default();
        let prepared = registry
            .prepare(&task("https://example.test/a.bin"))
            .unwrap();
        assert_eq!(prepared.url, "https://example.test/a.bin");
        assert!(prepared.headers.is_empty());
    }

    #[test]
    fn registry_rejects_unsupported_urls() {
        let registry = ProviderRegistry::default();
        let error = registry
            .prepare(&task("ftp://example.test/a.bin"))
            .unwrap_err();
        assert!(matches!(error, HttpError::NoProvider(url) if url == "ftp://example.test/a.bin"));
    }

    #[test]
    fn register_first_takes_precedence() {
        struct Stub;
        impl DownloadProvider for Stub {
            fn name(&self) -> &str {
                "stub"
            }
            fn can_handle(&self, _url: &str) -> bool {
                true
            }
            fn prepare(&self, _task: &DownloadTask) -> Result<PreparedDownload, HttpError> {
                Ok(PreparedDownload {
                    url: "https://signed.example/x".to_string(),
                    headers: vec![("Authorization".to_string(), "Bearer t".to_string())],
                })
            }
        }
        let mut registry = ProviderRegistry::default();
        registry.register(Box::new(Stub), true);
        let prepared = registry
            .prepare(&task("https://example.test/a.bin"))
            .unwrap();
        assert_eq!(prepared.url, "https://signed.example/x");
        assert_eq!(prepared.headers.len(), 1);
    }

    #[test]
    fn validates_and_restricts_provider_headers() {
        let prepared = PreparedDownload {
            url: "https://example.test/file".to_string(),
            headers: vec![("Authorization".to_string(), "Bearer token".to_string())],
        };
        assert_eq!(
            prepared
                .request_headers()
                .unwrap()
                .get("authorization")
                .unwrap(),
            "Bearer token"
        );

        for forbidden in ["Range", "If-Range", "Accept-Encoding"] {
            let prepared = PreparedDownload {
                url: "https://example.test/file".to_string(),
                headers: vec![(forbidden.to_string(), "value".to_string())],
            };
            assert!(matches!(
                prepared.request_headers(),
                Err(HttpError::ForbiddenHeader(_))
            ));
        }
    }
}
