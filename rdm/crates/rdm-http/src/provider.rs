//! Download providers. Port of the Python `providers` package.
//!
//! A provider turns a task's stored URL into the URL and headers actually used
//! for one engine run — the extension point for authenticated or signed
//! sources. Plain HTTP/HTTPS is handled by the built-in [`HttpDownloadProvider`].

use rdm_domain::DownloadTask;
use url::Url;

use crate::error::HttpError;

/// A provider-resolved URL and request headers for one download run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedDownload {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl PreparedDownload {
    /// A prepared download for `url` with no extra headers.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
        }
    }
}

/// Resolves a task into a [`PreparedDownload`]. Implementations must be
/// shareable across worker threads.
pub trait DownloadProvider: Send + Sync {
    fn name(&self) -> &str;
    fn can_handle(&self, url: &str) -> bool;
    fn prepare(&self, task: &DownloadTask) -> Result<PreparedDownload, HttpError>;
}

/// The built-in provider for plain `http`/`https` URLs.
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

/// An ordered list of providers; the first one that can handle a URL wins.
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
    /// A registry with an explicit provider list.
    pub fn new(providers: Vec<Box<dyn DownloadProvider>>) -> Self {
        Self { providers }
    }

    /// Add a provider, at the front (so it takes precedence) when `first`.
    pub fn register(&mut self, provider: Box<dyn DownloadProvider>, first: bool) {
        if first {
            self.providers.insert(0, provider);
        } else {
            self.providers.push(provider);
        }
    }

    /// Resolve `task` with the first matching provider.
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
}
