//! RDM 的媒体解析层。
//!
//! 将社交媒体 / 网页帖子的 URL 转换为 [`ResolvedPost`]——一个扁平化的、
//! 可直接下载的 [`MediaItem`] 列表,附带帖子正文文本,便于桌面应用将
//! 每个文件交给下载引擎处理。
//!
//! 默认已注册 Twitter / X、Instagram 和 Threads;实现 [`MediaResolver`]
//! 即可接入更多平台。

pub mod error;
pub mod instagram;
pub mod model;
pub mod resolver;
pub mod threads;
pub mod twitter;
pub mod util;

pub use error::ResolveError;
pub use instagram::InstagramResolver;
pub use model::{MediaItem, MediaKind, ResolvedPost};
pub use resolver::{MediaResolver, ProxyConfig, ResolverRegistry};
pub use threads::ThreadsResolver;
pub use twitter::TwitterResolver;
