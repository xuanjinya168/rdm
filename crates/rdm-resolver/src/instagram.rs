//! Instagram resolver.
//!
//! Port of ParseHub's `parsers/parser/instagram.py`, which delegates to
//! `instaloader`. Rather than reimplement that library we issue the same
//! anonymous request it makes for a single post: a GraphQL `doc_id` query to
//! `graphql/query` carrying the public web `x-ig-app-id`. The response's
//! `data.xdt_shortcode_media` node describes the post and its media.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ResolveError;
use crate::model::{MediaItem, MediaKind, ResolvedPost};
use crate::resolver::MediaResolver;
use crate::util::url_ext;

const GRAPHQL_URL: &str = "https://www.instagram.com/graphql/query";
/// Persisted-query id for the single-post query (same one instaloader uses).
const DOC_ID: &str = "8845758582119845";
/// Public web app id that unlocks anonymous reads of public posts.
const APP_ID: &str = "936619743392459";

/// URL path keywords that precede a post shortcode.
const SHORTCODE_KEYWORDS: [&str; 4] = ["p", "reel", "reels", "tv"];

/// Resolver for `instagram.com` post / reel / share links.
pub struct InstagramResolver;

impl InstagramResolver {
    pub fn new() -> Self {
        Self
    }

    /// Extract the post shortcode from a canonical post URL.
    ///
    /// Handles `/p/<code>`, `/reel(s)/<code>`, `/tv/<code>` and the
    /// `/<user>/p/<code>` variants. `/share/` links must be redirect-resolved
    /// first (see [`InstagramResolver::shortcode_for`]).
    fn shortcode(url: &str) -> Option<String> {
        let path = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .trim_end_matches('/');
        let segments: Vec<&str> = path.split('/').collect();
        for (i, segment) in segments.iter().enumerate() {
            if SHORTCODE_KEYWORDS.contains(segment) {
                return segments
                    .get(i + 1)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
            }
        }
        None
    }

    /// Resolve the shortcode, following the redirect that `/share/` links use.
    async fn shortcode_for(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<String, ResolveError> {
        if let Some(code) = Self::shortcode(url) {
            return Ok(code);
        }
        if url.contains("/share/") {
            // A share link 302s to the real post URL; follow it and retry.
            let response = client.get(url).send().await?.error_for_status()?;
            let final_url = response.url().as_str().to_string();
            if let Some(code) = Self::shortcode(&final_url) {
                return Ok(code);
            }
        }
        Err(ResolveError::InvalidUrl)
    }

    async fn fetch(
        &self,
        client: &reqwest::Client,
        shortcode: &str,
    ) -> Result<Value, ResolveError> {
        let variables = format!(r#"{{"shortcode":"{shortcode}"}}"#);
        let response = client
            .get(GRAPHQL_URL)
            .query(&[
                ("doc_id", DOC_ID),
                ("variables", variables.as_str()),
            ])
            .header("x-ig-app-id", APP_ID)
            .header("accept", "*/*")
            .header("referer", "https://www.instagram.com/")
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json().await?)
    }

    fn parse(&self, source_url: &str, shortcode: &str, body: &Value) -> Result<ResolvedPost, ResolveError> {
        let media_root = body
            .pointer("/data/xdt_shortcode_media")
            .filter(|v| !v.is_null())
            .ok_or_else(|| {
                ResolveError::Upstream("帖子不存在、已被删除或需要登录才能查看".into())
            })?;

        let text = media_root
            .pointer("/edge_media_to_caption/edges/0/node/text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let mut media = Vec::new();
        if let Some(edges) = media_root
            .pointer("/edge_sidecar_to_children/edges")
            .and_then(Value::as_array)
        {
            // Carousel: one media item per child node.
            for node in edges.iter().filter_map(|edge| edge.get("node")) {
                Self::push_node(&mut media, shortcode, node);
            }
        } else {
            // A single image or video lives on the root node itself.
            Self::push_node(&mut media, shortcode, media_root);
        }

        Ok(ResolvedPost {
            platform: "instagram".into(),
            source_url: source_url.to_string(),
            title: String::new(),
            text,
            media,
        })
    }

    /// Append the media described by one node (`xdt_shortcode_media` or a
    /// sidecar child) to `media`.
    fn push_node(media: &mut Vec<MediaItem>, shortcode: &str, node: &Value) {
        let is_video = node.get("is_video").and_then(Value::as_bool).unwrap_or(false);
        let width = node.pointer("/dimensions/width").and_then(Value::as_u64).unwrap_or(0) as u32;
        let height = node
            .pointer("/dimensions/height")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let index = media.len() + 1;
        let display_url = node.get("display_url").and_then(Value::as_str);

        if is_video {
            let Some(url) = node.get("video_url").and_then(Value::as_str) else {
                return;
            };
            let duration = node
                .get("video_duration")
                .and_then(Value::as_f64)
                .map(|secs| secs.round() as u32);
            media.push(MediaItem {
                kind: MediaKind::Video,
                url: url.to_string(),
                thumb_url: display_url.map(str::to_owned),
                width,
                height,
                duration_secs: duration,
                ext: "mp4".into(),
                filename: format!("{shortcode}_{index}.mp4"),
            });
        } else {
            let Some(url) = display_url else {
                return;
            };
            let ext = url_ext(url, "jpg");
            media.push(MediaItem {
                kind: MediaKind::Image,
                url: url.to_string(),
                thumb_url: Some(url.to_string()),
                width,
                height,
                duration_secs: None,
                filename: format!("{shortcode}_{index}.{ext}"),
                ext,
            });
        }
    }
}

impl Default for InstagramResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MediaResolver for InstagramResolver {
    fn name(&self) -> &str {
        "instagram"
    }

    fn can_handle(&self, url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        if !lower.contains("instagram.com/") {
            return false;
        }
        ["/p/", "/reel/", "/reels/", "/tv/", "/share/"]
            .iter()
            .any(|seg| lower.contains(seg))
    }

    async fn resolve(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ResolvedPost, ResolveError> {
        let shortcode = self.shortcode_for(client, url).await?;
        let body = self.fetch(client, &shortcode).await?;
        let post = self.parse(url, &shortcode, &body)?;
        if post.media.is_empty() {
            return Err(ResolveError::NoMedia);
        }
        Ok(post)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_shortcode_from_post_reel_and_user_paths() {
        assert_eq!(
            InstagramResolver::shortcode("https://www.instagram.com/p/CabcDEF/"),
            Some("CabcDEF".into())
        );
        assert_eq!(
            InstagramResolver::shortcode("https://instagram.com/reel/XyZ123?utm=1"),
            Some("XyZ123".into())
        );
        assert_eq!(
            InstagramResolver::shortcode("https://www.instagram.com/someuser/p/AAA111/"),
            Some("AAA111".into())
        );
        assert_eq!(InstagramResolver::shortcode("https://www.instagram.com/someuser/"), None);
    }

    #[test]
    fn can_handle_matches_instagram_media_links() {
        let r = InstagramResolver::new();
        assert!(r.can_handle("https://www.instagram.com/p/CabcDEF/"));
        assert!(r.can_handle("https://www.instagram.com/reel/XyZ/"));
        assert!(r.can_handle("https://www.instagram.com/share/AbC/"));
        assert!(!r.can_handle("https://www.instagram.com/someuser/"));
        assert!(!r.can_handle("https://example.com/p/CabcDEF/"));
    }

    #[test]
    fn parses_single_image_post() {
        let body = serde_json::json!({
            "data": {"xdt_shortcode_media": {
                "is_video": false,
                "display_url": "https://scontent.cdninstagram.com/v/a.jpg?stp=x",
                "dimensions": {"width": 1080, "height": 1350},
                "edge_media_to_caption": {"edges": [{"node": {"text": "hello"}}]}
            }}
        });
        let post = InstagramResolver::new()
            .parse("https://www.instagram.com/p/AAA/", "AAA", &body)
            .unwrap();
        assert_eq!(post.text, "hello");
        assert_eq!(post.media.len(), 1);
        assert_eq!(post.media[0].kind, MediaKind::Image);
        assert_eq!(post.media[0].ext, "jpg");
        assert_eq!(post.media[0].filename, "AAA_1.jpg");
    }

    #[test]
    fn parses_carousel_with_image_and_video() {
        let body = serde_json::json!({
            "data": {"xdt_shortcode_media": {
                "edge_media_to_caption": {"edges": []},
                "edge_sidecar_to_children": {"edges": [
                    {"node": {
                        "is_video": false,
                        "display_url": "https://cdn/i1.jpg",
                        "dimensions": {"width": 640, "height": 640}
                    }},
                    {"node": {
                        "is_video": true,
                        "display_url": "https://cdn/thumb.jpg",
                        "video_url": "https://cdn/clip.mp4",
                        "video_duration": 12.7,
                        "dimensions": {"width": 720, "height": 1280}
                    }}
                ]}
            }}
        });
        let post = InstagramResolver::new()
            .parse("https://www.instagram.com/p/BBB/", "BBB", &body)
            .unwrap();
        assert_eq!(post.media.len(), 2);
        assert_eq!(post.media[0].kind, MediaKind::Image);
        assert_eq!(post.media[0].filename, "BBB_1.jpg");
        let video = &post.media[1];
        assert_eq!(video.kind, MediaKind::Video);
        assert_eq!(video.url, "https://cdn/clip.mp4");
        assert_eq!(video.thumb_url.as_deref(), Some("https://cdn/thumb.jpg"));
        assert_eq!(video.duration_secs, Some(13));
        assert_eq!(video.filename, "BBB_2.mp4");
    }

    #[test]
    fn missing_media_node_is_upstream_error() {
        let body = serde_json::json!({ "data": { "xdt_shortcode_media": null } });
        let err = InstagramResolver::new()
            .parse("https://www.instagram.com/p/CCC/", "CCC", &body)
            .unwrap_err();
        assert!(matches!(err, ResolveError::Upstream(_)));
    }
}
