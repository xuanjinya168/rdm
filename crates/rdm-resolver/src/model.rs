//! 解析器产生的平台无关结果类型。
//!
//! 这些镜像了 ParseHub `types` 包的相关部分（`MediaRef`、`Post`），
//! 但被扁平化为桌面 UI 可直接渲染、下载引擎可逐项消费的形状。

use serde::Serialize;

/// 单个已解析媒体文件的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// 静态图片。
    Image,
    /// 带音频的视频。
    Video,
    /// 循环静音片段（Twitter "animated_gif"），以 mp4 格式交付。
    Gif,
}

/// 从帖子中提取的单个可下载文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaItem {
    pub kind: MediaKind,
    /// 要下载的最高质量变体的直接 URL。
    pub url: String,
    /// 原始媒体的像素宽度（若已知，未知时为 `0`）。
    pub width: u32,
    /// 原始媒体的像素高度（若已知，未知时为 `0`）。
    pub height: u32,
    /// 视频/gif 的时长（整秒）。
    pub duration_secs: Option<u32>,
    /// 文件扩展名（不带点，例如 `mp4`、`jpg`）。
    pub ext: String,
    /// 下载的安全、人类可读文件名建议。
    pub filename: String,
}

/// 解析单个帖子 URL 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedPost {
    /// 源平台的标识符（例如 `twitter`）。
    pub platform: String,
    /// 被解析的规范 URL。
    pub source_url: String,
    /// 可选的帖子标题（文章）；普通推文为空。
    pub title: String,
    /// 帖子的文本正文。
    pub text: String,
    /// 帖子中找到的每个可下载媒体文件。
    pub media: Vec<MediaItem>,
}
