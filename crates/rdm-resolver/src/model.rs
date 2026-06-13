//! Platform-agnostic result types produced by a resolver.
//!
//! These mirror the relevant parts of ParseHub's `types` package (`MediaRef`,
//! `Post`) but are flattened into a shape the desktop UI can render directly and
//! the download engine can consume one item at a time.

use serde::Serialize;

/// The kind of a single resolved media file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// A still image.
    Image,
    /// A video with audio.
    Video,
    /// A looping, silent clip (Twitter "animated_gif"), delivered as mp4.
    Gif,
}

impl MediaKind {
    /// Short label used by the UI.
    pub fn label(self) -> &'static str {
        match self {
            MediaKind::Image => "图片",
            MediaKind::Video => "视频",
            MediaKind::Gif => "动图",
        }
    }
}

/// One downloadable file extracted from a post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaItem {
    pub kind: MediaKind,
    /// Direct URL of the highest-quality variant to download.
    pub url: String,
    /// Preview/thumbnail URL, when the platform provides one.
    pub thumb_url: Option<String>,
    /// Pixel width of the original media, if known (`0` when unknown).
    pub width: u32,
    /// Pixel height of the original media, if known (`0` when unknown).
    pub height: u32,
    /// Duration in whole seconds, for video/gif.
    pub duration_secs: Option<u32>,
    /// File extension without the dot (e.g. `mp4`, `jpg`).
    pub ext: String,
    /// A safe, human-readable filename suggestion for the download.
    pub filename: String,
}

/// The result of resolving a single post URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedPost {
    /// Identifier of the source platform (e.g. `twitter`).
    pub platform: String,
    /// The canonical URL that was resolved.
    pub source_url: String,
    /// Optional post title (articles); empty for plain tweets.
    pub title: String,
    /// The post's text body.
    pub text: String,
    /// Every downloadable media file found in the post.
    pub media: Vec<MediaItem>,
}
