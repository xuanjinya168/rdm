//! Media resolver layer for RDM.
//!
//! Turns a social-media / web post URL into a [`ResolvedPost`] — a flat list of
//! directly downloadable [`MediaItem`]s plus the post text — so the desktop app
//! can preview them and hand each file to the download engine.
//!
//! Ported from the Python project ParseHub (<https://github.com/z-mio/ParseHub>).
//! Twitter / X is implemented today; other platforms plug in by adding another
//! [`MediaResolver`] in [`ResolverRegistry::new`].

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
