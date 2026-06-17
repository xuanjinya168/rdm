//! The resolver trait and the registry that dispatches a URL to one.

use async_trait::async_trait;

use crate::error::ResolveError;
use crate::instagram::InstagramResolver;
use crate::model::ResolvedPost;
use crate::threads::ThreadsResolver;
use crate::twitter::TwitterResolver;

/// Optional proxy configuration applied to the media-resolver client.
///
/// Mirrors `rdm_http::ProxyConfig`; duplicated here to keep `rdm-resolver`
/// independent of the download HTTP layer. `url` may be `http://`, `https://`
/// or `socks5://`.
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

/// A per-platform resolver. Implementations turn a post URL into the set of
/// media files it contains. They must be shareable across worker threads.
#[async_trait]
pub trait MediaResolver: Send + Sync {
    /// Stable identifier of the platform this resolver serves.
    fn name(&self) -> &str;

    /// Whether this resolver recognises `url`.
    fn can_handle(&self, url: &str) -> bool;

    /// Resolve `url` into a post using the shared `client`.
    async fn resolve(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ResolvedPost, ResolveError>;
}

/// Holds the registered resolvers and a shared HTTP client, dispatching each URL
/// to the first resolver that claims it. New platforms are added by pushing
/// another [`MediaResolver`] in [`ResolverRegistry::new`].
pub struct ResolverRegistry {
    client: reqwest::Client,
    resolvers: Vec<Box<dyn MediaResolver>>,
}

impl ResolverRegistry {
    /// Build the registry with every supported platform wired in.
    ///
    /// `proxy` optionally routes every request through a proxy. The URL may use
    /// the `http://`, `https://` or `socks5://` scheme; when `username` is
    /// non-empty it is sent as Basic auth. A proxy that reqwest cannot parse is
    /// logged and skipped so resolution still works over a direct connection.
    pub fn new(proxy: ProxyConfig) -> Result<Self, ResolveError> {
        // A browser-like UA: some endpoints reject the default reqwest agent.
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
                    "Ignoring invalid media-resolver proxy {:?}: {error}",
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

    /// Whether any registered resolver can handle `url`.
    pub fn can_resolve(&self, url: &str) -> bool {
        self.resolvers.iter().any(|r| r.can_handle(url))
    }

    /// Resolve `url`, or [`ResolveError::Unsupported`] if no resolver matches.
    pub async fn resolve(&self, url: &str) -> Result<ResolvedPost, ResolveError> {
        let resolver = self
            .resolvers
            .iter()
            .find(|r| r.can_handle(url))
            .ok_or(ResolveError::Unsupported)?;
        resolver.resolve(&self.client, url).await
    }
}
