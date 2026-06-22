//! Threads 解析器。
//!
//! 移植自 ParseHub 的 `provider_api/threads.py` + `parsers/parser/threads.py`。
//! Threads 没有公开 API，因此我们复制其 web 客户端的匿名调用：
//! 一个携带随机 `lsd` token 的表单 POST 到 `ajax/route-definition`。
//! 回复是 `for (;;);` 前缀的 JSON 对象流；说明来自 `first_response` 对象，
//! 媒体来自 Barcelona lightbox 的 `preloader` 对象。

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ResolveError;
use crate::model::{MediaItem, MediaKind, ResolvedPost};
use crate::resolver::MediaResolver;
use crate::util::url_ext;

const ROUTE_URL: &str = "https://www.threads.com/ajax/route-definition";
/// 在预加载 `id` 中标识帖子媒体负载的标记常量。
const MEDIA_PRELOADER: &str = "BarcelonaLightboxDialogRootQueryRelayPreloader";

/// 解析 `threads.com` / `threads.net` 帖子链接。
pub struct ThreadsResolver;

impl ThreadsResolver {
    pub fn new() -> Self {
        Self
    }

    /// 从 `/@user/post/<id>` URL 中提取 `(@username, post_id)`。
    fn locate(url: &str) -> Option<(String, String)> {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let segments: Vec<&str> = path.split('/').collect();
        for (i, segment) in segments.iter().enumerate() {
            if segment.starts_with('@') && segment.len() > 1 && segments.get(i + 1) == Some(&"post")
            {
                let post_id = segments.get(i + 2).filter(|p| !p.is_empty())?;
                return Some((segment.to_string(), post_id.to_string()));
            }
        }
        None
    }

    /// 类似 web 客户端生成的随机字母数字 `lsd` token。
    fn random_lsd() -> String {
        uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(11)
            .collect()
    }

    async fn fetch(
        &self,
        client: &reqwest::Client,
        username: &str,
        post_id: &str,
    ) -> Result<Vec<Value>, ResolveError> {
        let lsd = Self::random_lsd();
        let route_url = format!("/{username}/post/{post_id}/media");
        let form = [
            ("route_url", route_url.as_str()),
            ("routing_namespace", "barcelona_web"),
            ("__user", "0"),
            ("__a", "1"),
            ("__req", "m"),
            ("__comet_req", "29"),
            ("lsd", lsd.as_str()),
        ];

        let text = client
            .post(ROUTE_URL)
            .header("sec-fetch-site", "same-origin")
            .header("x-fb-lsd", &lsd)
            .form(&form)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(split_jsonp(&text))
    }

    fn parse(&self, source_url: &str, post_id: &str, objects: &[Value]) -> ResolvedPost {
        let mut text = String::new();
        let mut media = Vec::new();
        for object in objects {
            match object.get("__type").and_then(Value::as_str) {
                Some("first_response") => {
                    if let Some(caption) = fetch_content(object) {
                        text = caption;
                    }
                }
                Some("preloader") => {
                    let id = object.get("id").and_then(Value::as_str).unwrap_or("");
                    if id.contains(MEDIA_PRELOADER) {
                        if let Some(node) = object.pointer("/result/result/data/data") {
                            collect_media(post_id, node, &mut media);
                        }
                    }
                }
                _ => {}
            }
        }
        ResolvedPost {
            platform: "threads".into(),
            source_url: source_url.to_string(),
            title: String::new(),
            text,
            media,
        }
    }
}

impl Default for ThreadsResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MediaResolver for ThreadsResolver {
    fn name(&self) -> &str {
        "threads"
    }

    fn can_handle(&self, url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        (lower.contains("threads.com/") || lower.contains("threads.net/"))
            && lower.contains("/@")
            && lower.contains("/post/")
    }

    async fn resolve(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ResolvedPost, ResolveError> {
        let (username, post_id) = Self::locate(url).ok_or(ResolveError::InvalidUrl)?;
        let objects = self.fetch(client, &username, &post_id).await?;
        if objects.is_empty() {
            return Err(ResolveError::Upstream(
                "无法获取帖子内容，可能是私密帖子或需要登录".into(),
            ));
        }
        let post = self.parse(url, &post_id, &objects);
        if post.media.is_empty() {
            return Err(ResolveError::NoMedia);
        }
        Ok(post)
    }
}

/// 将 Facebook/Threads 的 `for (;;);` 分隔 JSONP 正文拆分为 JSON 对象，
/// 跳过任何无法单独解析的块。
fn split_jsonp(text: &str) -> Vec<Value> {
    text.split("for (;;);")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| serde_json::from_str::<Value>(chunk).ok())
        .collect()
}

/// 从 `first_response` 对象读取说明，处理用户名变更后使用的
/// `redirect_result` 形状。
fn fetch_content(object: &Value) -> Option<String> {
    let result = object.pointer("/payload/result")?;
    result
        .pointer("/redirect_result/exports/meta/title")
        .or_else(|| result.pointer("/exports/meta/title"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// 按 `media_type` 遍历媒体节点并追加其产生的每个文件。类型 19
/// （嵌入链接媒体的文本帖子）会递归。
fn collect_media(post_id: &str, node: &Value, media: &mut Vec<MediaItem>) {
    match node.get("media_type").and_then(Value::as_i64) {
        Some(1) => {
            if let Some(item) = image_item(node, post_id, media.len() + 1) {
                media.push(item);
            }
        }
        Some(2) => {
            if let Some(item) = video_item(node, post_id, media.len() + 1) {
                media.push(item);
            }
        }
        Some(8) => {
            let Some(children) = node.get("carousel_media").and_then(Value::as_array) else {
                return;
            };
            for child in children {
                let has_video = child
                    .get("video_versions")
                    .and_then(Value::as_array)
                    .is_some_and(|v| !v.is_empty());
                let item = if has_video {
                    video_item(child, post_id, media.len() + 1)
                } else {
                    image_item(child, post_id, media.len() + 1)
                };
                if let Some(item) = item {
                    media.push(item);
                }
            }
        }
        Some(19) => {
            if let Some(linked) = node
                .pointer("/text_post_app_info/linked_inline_media")
                .filter(|v| !v.is_null())
            {
                collect_media(post_id, linked, media);
            }
        }
        _ => {}
    }
}

fn image_item(node: &Value, post_id: &str, index: usize) -> Option<MediaItem> {
    let candidate = node.pointer("/image_versions2/candidates/0")?;
    let url = candidate.get("url").and_then(Value::as_str)?;
    let (width, height) = dimensions(node, candidate);
    let ext = url_ext(url, "jpg");
    Some(MediaItem {
        kind: MediaKind::Image,
        url: url.to_string(),
        width,
        height,
        duration_secs: None,
        filename: format!("{post_id}_{index}.{ext}"),
        ext,
    })
}

fn video_item(node: &Value, post_id: &str, index: usize) -> Option<MediaItem> {
    let url = node
        .pointer("/video_versions/0/url")
        .and_then(Value::as_str)?;
    let width = node
        .get("original_width")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let height = node
        .get("original_height")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let ext = url_ext(url, "mp4");
    Some(MediaItem {
        kind: MediaKind::Video,
        url: url.to_string(),
        width,
        height,
        duration_secs: None,
        filename: format!("{post_id}_{index}.{ext}"),
        ext,
    })
}

/// 优先使用图片候选自身的尺寸，回退到节点的
/// `original_width`/`original_height`。
fn dimensions(node: &Value, candidate: &Value) -> (u32, u32) {
    let width = candidate
        .get("width")
        .and_then(Value::as_u64)
        .or_else(|| node.get("original_width").and_then(Value::as_u64))
        .unwrap_or(0) as u32;
    let height = candidate
        .get("height")
        .and_then(Value::as_u64)
        .or_else(|| node.get("original_height").and_then(Value::as_u64))
        .unwrap_or(0) as u32;
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_username_and_post_id() {
        assert_eq!(
            ThreadsResolver::locate("https://www.threads.com/@user.name/post/AbC-123/"),
            Some(("@user.name".into(), "AbC-123".into()))
        );
        assert_eq!(
            ThreadsResolver::locate("https://www.threads.net/@jack/post/XYZ?x=1"),
            Some(("@jack".into(), "XYZ".into()))
        );
        assert_eq!(
            ThreadsResolver::locate("https://www.threads.com/@jack"),
            None
        );
    }

    #[test]
    fn can_handle_matches_threads_post_links() {
        let r = ThreadsResolver::new();
        assert!(r.can_handle("https://www.threads.com/@jack/post/ABC/"));
        assert!(r.can_handle("https://www.threads.net/@jack/post/ABC"));
        assert!(!r.can_handle("https://www.threads.com/@jack"));
        assert!(!r.can_handle("https://example.com/@jack/post/ABC"));
    }

    #[test]
    fn random_lsd_is_eleven_alphanumeric_chars() {
        let lsd = ThreadsResolver::random_lsd();
        assert_eq!(lsd.len(), 11);
        assert!(lsd.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn split_jsonp_extracts_each_object() {
        let body = r#"for (;;);{"__type":"a","v":1}for (;;);{"__type":"b","v":2}"#;
        let objects = split_jsonp(body);
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0]["__type"], "a");
        assert_eq!(objects[1]["v"], 2);
    }

    #[test]
    fn parses_caption_and_carousel() {
        let objects = vec![
            serde_json::json!({
                "__type": "first_response",
                "payload": {"result": {"exports": {"meta": {"title": "hello threads"}}}}
            }),
            serde_json::json!({
                "__type": "preloader",
                "id": "adp_BarcelonaLightboxDialogRootQueryRelayPreloader_abc",
                "result": {"result": {"data": {"data": {
                    "media_type": 8,
                    "carousel_media": [
                        {
                            "image_versions2": {"candidates": [{"url": "https://cdn/i.jpg", "width": 640, "height": 640}]},
                            "video_versions": [],
                            "original_width": 640, "original_height": 640
                        },
                        {
                            "image_versions2": {"candidates": [{"url": "https://cdn/t.jpg"}]},
                            "video_versions": [{"url": "https://cdn/v.mp4"}],
                            "original_width": 720, "original_height": 1280
                        }
                    ]
                }}}}
            }),
        ];
        let post =
            ThreadsResolver::new().parse("https://www.threads.com/@u/post/PID/", "PID", &objects);
        assert_eq!(post.text, "hello threads");
        assert_eq!(post.media.len(), 2);
        assert_eq!(post.media[0].kind, MediaKind::Image);
        assert_eq!(post.media[0].url, "https://cdn/i.jpg");
        assert_eq!(post.media[0].filename, "PID_1.jpg");
        let video = &post.media[1];
        assert_eq!(video.kind, MediaKind::Video);
        assert_eq!(video.url, "https://cdn/v.mp4");
        assert_eq!(video.width, 720);
        assert_eq!(video.filename, "PID_2.mp4");
    }

    #[test]
    fn parses_single_video_post() {
        let objects = vec![serde_json::json!({
            "__type": "preloader",
            "id": "x_BarcelonaLightboxDialogRootQueryRelayPreloader",
            "result": {"result": {"data": {"data": {
                "media_type": 2,
                "image_versions2": {"candidates": [{"url": "https://cdn/thumb.jpg"}]},
                "video_versions": [{"url": "https://cdn/clip.mp4"}],
                "original_width": 1080, "original_height": 1920
            }}}}
        })];
        let post =
            ThreadsResolver::new().parse("https://www.threads.com/@u/post/V/", "V", &objects);
        assert_eq!(post.media.len(), 1);
        assert_eq!(post.media[0].kind, MediaKind::Video);
        assert_eq!(post.media[0].url, "https://cdn/clip.mp4");
    }
}
