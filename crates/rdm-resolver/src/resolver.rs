//! 解析器 trait 与将 URL 分发给它的注册表。

use async_trait::async_trait;

use crate::error::ResolveError;
use crate::instagram::InstagramResolver;
use crate::model::ResolvedPost;
use crate::threads::ThreadsResolver;
use crate::twitter::TwitterResolver;

/// 应用于媒体解析客户端的可选代理配置。
///
/// 与 `rdm_http::ProxyConfig` 镜像；在此重复以保持 `rdm-resolver` 独立于
/// 下载 HTTP 层。`url` 可以是 `http://`、`https://` 或 `socks5://`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

impl ProxyConfig {
    /// 此配置是否实际请求代理。
    pub fn is_active(&self) -> bool {
        !self.url.trim().is_empty()
    }
}

/// 每平台解析器。实现将帖子 URL 转换为其包含的媒体文件集合。
/// 它们必须可跨工作线程共享。
#[async_trait]
pub trait MediaResolver: Send + Sync {
    /// 此解析器服务的平台的稳定标识符。
    fn name(&self) -> &str;

    /// 此解析器是否识别 `url`。
    fn can_handle(&self, url: &str) -> bool;

    /// 使用共享的 `client` 将 `url` 解析为帖子。
    async fn resolve(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ResolvedPost, ResolveError>;
}

/// 持有已注册的解析器与共享的 HTTP 客户端，将每个 URL 分发给
/// 第一个认领它的解析器。新平台通过在 [`ResolverRegistry::new`] 中
/// 推入另一个 [`MediaResolver`] 来添加。
pub struct ResolverRegistry {
    client: reqwest::Client,
    resolvers: Vec<Box<dyn MediaResolver>>,
}

impl ResolverRegistry {
    /// 构建已连接所有支持平台的注册表。
    ///
    /// `proxy` 可选地将每个请求通过代理路由。URL 可以使用
    /// `http://`、`https://` 或 `socks5://` 方案；当 `username` 非空时，
    /// 它作为 Basic auth 发送。reqwest 无法解析的代理会被记录并跳过，
    /// 因此解析仍可通过直接连接工作。
    pub fn new(proxy: ProxyConfig) -> Result<Self, ResolveError> {
        // 浏览器般的 UA：某些端点拒绝默认的 reqwest agent。
        let mut builder = reqwest::Client::builder().user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        );
        if proxy.is_active() {
            match reqwest::Proxy::all(&proxy.url) {
                Ok(mut p) => {
                    if !proxy.username.is_empty() {
                        p = p.basic_auth(&proxy.username, &proxy.password);
                    }
                    builder = builder.proxy(p);
                }
                Err(error) => log::warn!(
                    "忽略无效的媒体解析器代理 {:?}: {error}",
                    proxy.url
                ),
            }
        }
        let client = builder.build()?;
        Ok(Self {
            client,
            resolvers: vec![
                Box::new(TwitterResolver::new()),
                Box::new(InstagramResolver::new()),
                Box::new(ThreadsResolver::new()),
            ],
        })
    }

    /// 解析 `url`，若无解析器匹配则返回 [`ResolveError::Unsupported`]。
    pub async fn resolve(&self, url: &str) -> Result<ResolvedPost, ResolveError> {
        let resolver = self
            .resolvers
            .iter()
            .find(|r| r.can_handle(url))
            .ok_or(ResolveError::Unsupported)?;
        resolver.resolve(&self.client, url).await
    }
}
