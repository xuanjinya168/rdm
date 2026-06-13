//! The resolver trait and the registry that dispatches a URL to one.

use async_trait::async_trait;

use crate::error::ResolveError;
use crate::instagram::InstagramResolver;
use crate::model::ResolvedPost;
use crate::threads::ThreadsResolver;
use crate::twitter::TwitterResolver;

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
    pub fn new() -> Result<Self, ResolveError> {
        // A browser-like UA: some endpoints reject the default reqwest agent.
        let client = reqwest::Client::builder()
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            )
            .build()?;
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
