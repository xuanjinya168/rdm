//! Construction of the shared reqwest client.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};

use crate::error::HttpError;

/// Sent as the `User-Agent` on every request.
pub const USER_AGENT: &str = concat!("RDM/", env!("CARGO_PKG_VERSION"));

/// Optional proxy configuration applied when building a client.
///
/// `url` may use the `http://`, `https://` or `socks5://` schemes. When
/// `username` is non-empty the credentials are sent as Basic auth. The value is
/// stored already-trimmed by [`AppSettings::validated`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

impl ProxyConfig {
    /// Whether this configuration actually requests a proxy.
    pub fn is_active(&self) -> bool {
        !self.url.trim().is_empty()
    }
}

/// Configure `builder` with the proxy described by `proxy`, if any. A proxy URL
/// that reqwest cannot parse is logged and silently skipped so that a typo does
/// not break every download.
fn apply_proxy(mut builder: reqwest::ClientBuilder, proxy: &ProxyConfig) -> reqwest::ClientBuilder {
    if !proxy.is_active() {
        return builder;
    }
    match reqwest::Proxy::all(&proxy.url) {
        Ok(mut p) => {
            if !proxy.username.is_empty() {
                p = p.basic_auth(&proxy.username, &proxy.password);
            }
            builder = builder.proxy(p);
        }
        Err(error) => log::warn!(
            "Ignoring invalid proxy {:?}: {error}; falling back to direct connection",
            proxy.url
        ),
    }
    builder
}

/// Build a client sized for `connections` parallel segment requests.
///
/// Mirrors the Python engine's httpx setup: identity encoding (so byte ranges
/// map straight to file offsets), redirect following, and a pool a little
/// larger than the worker count. There is deliberately no total-request
/// timeout — a multi-gigabyte download must not be killed mid-stream — only a
/// connect timeout and a per-read inactivity timeout.
///
/// When `proxy` is active (`ProxyConfig::is_active`), every request is routed
/// through it; otherwise reqwest's defaults (including any `HTTP_PROXY` /
/// `HTTPS_PROXY` environment variables) apply.
pub fn build_client(connections: u32, proxy: &ProxyConfig) -> Result<reqwest::Client, HttpError> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

    let builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .pool_max_idle_per_host(connections as usize + 4)
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(30));
    apply_proxy(builder, proxy).build().map_err(HttpError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_without_proxy() {
        // No real connection is opened; building only wires configuration.
        let client = build_client(8, &ProxyConfig::default()).unwrap();
        assert!(client.get("http://example.invalid").build().is_ok());
    }

    #[test]
    fn builds_with_http_proxy() {
        let proxy = ProxyConfig {
            url: "http://127.0.0.1:7890".to_string(),
            ..ProxyConfig::default()
        };
        assert!(build_client(8, &proxy).is_ok());
    }

    #[test]
    fn builds_with_socks5_proxy() {
        let proxy = ProxyConfig {
            url: "socks5://127.0.0.1:1080".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        assert!(build_client(8, &proxy).is_ok());
    }

    #[test]
    fn invalid_proxy_url_falls_back_to_a_buildable_client() {
        // reqwest rejects a URL with no scheme; we skip the proxy rather than
        // failing the whole client build.
        let proxy = ProxyConfig {
            url: "not-a-valid-url".to_string(),
            ..ProxyConfig::default()
        };
        assert!(build_client(8, &proxy).is_ok());
    }

    #[test]
    fn proxy_config_activity() {
        assert!(!ProxyConfig::default().is_active());
        assert!(!ProxyConfig {
            url: "   ".to_string(),
            ..ProxyConfig::default()
        }
        .is_active());
        assert!(ProxyConfig {
            url: "http://x".to_string(),
            ..ProxyConfig::default()
        }
        .is_active());
    }
}
