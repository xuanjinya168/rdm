//! 共享 reqwest 客户端的构造。

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};

use crate::error::HttpError;

/// 每次请求发送的 `User-Agent`。
pub const USER_AGENT: &str = concat!("RDM/", env!("CARGO_PKG_VERSION"));

/// 构造客户端时应用的可选代理配置。
///
/// `url` 可使用 `http://`、`https://` 或 `socks5://` 协议。
/// 当 `username` 非空时，会以 Basic 认证发送凭据。值在
/// [`AppSettings::validated`] 中已预先去除首尾空白。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

impl ProxyConfig {
    /// 该配置是否实际启用了代理。
    pub fn is_active(&self) -> bool {
        !self.url.trim().is_empty()
    }
}

/// 使用 `proxy` 描述的代理（若存在）配置 `builder`。
/// 对于 reqwest 无法解析的代理 URL 仅记录日志并静默跳过，
/// 以避免一个笔误就破坏所有下载。
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

/// 为 `connections` 个并发的分段请求构建客户端。
///
/// 复刻 Python 引擎的 httpx 配置：使用 identity 编码（让字节区间
/// 与文件偏移一一对应）、跟随重定向，并将连接池略大于工作线程数。
/// 故意不设置总请求超时 —— 多 GB 的下载不应在中途被中断；
/// 仅设置连接超时与每次读取的不活跃超时。
///
/// 当 `proxy` 处于激活状态（[`ProxyConfig::is_active`]）时，
/// 所有请求都会经由代理；否则使用 reqwest 的默认行为
/// （包括 `HTTP_PROXY` / `HTTPS_PROXY` 等环境变量）。
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
        // 未打开真实连接；构建仅用于配置的拼接。
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
        // reqwest 拒绝无协议的 URL；我们跳过代理，而非使整个客户端构建失败。
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
