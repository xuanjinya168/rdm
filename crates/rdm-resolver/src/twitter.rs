//! Twitter / X 解析器。
//!
//! 移植自 ParseHub 的 `provider_api/twitter.py` + `parsers/parser/twitter.py`。
//! 它通过公开的 GraphQL `TweetResultByRestId` 端点使用已知的 web bearer token
//! 读取单个推文。与 Python 参考实现不同，我们首先激活访客 token
//! （`/1.1/guest/activate.json`），端点要求匿名读取时使用该 token。

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ResolveError;
use crate::model::{MediaItem, MediaKind, ResolvedPost};
use crate::resolver::MediaResolver;
use crate::util::url_ext;

/// twitter.com 提供的公共 web 应用 bearer token（ParseHub 和 yt-dlp 使用的相同值）。
/// 它仅授予公开匿名读取的权限。
const BEARER: &str = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

const GUEST_ACTIVATE_URL: &str = "https://api.twitter.com/1.1/guest/activate.json";
const TWEET_URL: &str =
    "https://api.twitter.com/graphql/kPLTRmMnzbPTv70___D06w/TweetResultByRestId";

/// GraphQL `features` 标志集，固定查询 ID 期望的值。从参考实现逐字复制；
/// 端点会在缺少标志时拒绝请求。
const FEATURES: &str = r#"{"creator_subscriptions_tweet_preview_api_enabled":true,"communities_web_enable_tweet_community_results_fetch":true,"c9s_tweet_anatomy_moderator_badge_enabled":true,"tweetypie_unmention_optimization_enabled":true,"responsive_web_edit_tweet_api_enabled":true,"graphql_is_translatable_rweb_tweet_is_translatable_enabled":true,"view_counts_everywhere_api_enabled":true,"longform_notetweets_consumption_enabled":true,"responsive_web_twitter_article_tweet_consumption_enabled":true,"tweet_awards_web_tipping_enabled":false,"creator_subscriptions_quote_tweet_preview_enabled":false,"freedom_of_speech_not_reach_fetch_enabled":true,"standardized_nudges_misinfo":true,"tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled":true,"tweet_with_visibility_results_prefer_gql_media_interstitial_enabled":false,"rweb_video_timestamps_enabled":true,"longform_notetweets_rich_text_read_enabled":true,"longform_notetweets_inline_media_enabled":true,"rweb_tipjar_consumption_enabled":true,"responsive_web_graphql_exclude_directive_enabled":true,"verified_phone_label_enabled":false,"responsive_web_graphql_skip_user_profile_image_extensions_enabled":false,"responsive_web_graphql_timeline_navigation_enabled":true,"responsive_web_enhance_cards_enabled":false}"#;

const FIELD_TOGGLES: &str = r#"{"withArticleRichContentState":true,"withArticlePlainText":false}"#;

/// 解析 `twitter.com` / `x.com` / `fixupx.com` 的状态链接。
pub struct TwitterResolver;

impl TwitterResolver {
    pub fn new() -> Self {
        Self
    }

    /// 从任何 `.../status/<id>` URL 中提取数字推文 ID。
    fn tweet_id(url: &str) -> Option<String> {
        let rest = url.split("status/").nth(1)?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            None
        } else {
            Some(digits)
        }
    }

    /// 请求用于匿名读取的短时访客 token。
    async fn guest_token(&self, client: &reqwest::Client) -> Result<String, ResolveError> {
        let response = client
            .post(GUEST_ACTIVATE_URL)
            .header("authorization", BEARER)
            .send()
            .await?
            .error_for_status()?;
        let body: Value = response.json().await?;
        body.get("guest_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ResolveError::Decode("响应缺少 guest_token".into()))
    }

    async fn fetch(&self, client: &reqwest::Client, tweet_id: &str) -> Result<Value, ResolveError> {
        let guest_token = self.guest_token(client).await?;
        let variables = format!(
            r#"{{"tweetId":"{tweet_id}","withCommunity":false,"includePromotedContent":false,"withVoice":false}}"#
        );

        let response = client
            .get(TWEET_URL)
            .query(&[
                ("variables", variables.as_str()),
                ("features", FEATURES),
                ("fieldToggles", FIELD_TOGGLES),
            ])
            .header("authorization", BEARER)
            .header("content-type", "application/json")
            .header("x-guest-token", guest_token)
            .header("x-twitter-active-user", "yes")
            .header("x-twitter-client-language", "en")
            .header("accept-language", "en-US,en;q=0.9")
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json().await?)
    }

    /// 将 GraphQL 响应转换为 [`ResolvedPost`]。镜像了参考实现中的
    /// `Twitter.parse`。
    fn parse(&self, source_url: &str, body: &Value) -> Result<ResolvedPost, ResolveError> {
        if let Some(errors) = body.get("errors").and_then(Value::as_array) {
            if let Some(message) = errors
                .first()
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
            {
                return Err(ResolveError::Upstream(message.to_string()));
            }
        }

        let result = body
            .pointer("/data/tweetResult/result")
            .filter(|v| !v.is_null())
            .ok_or_else(|| ResolveError::Upstream("帖子或用户不存在".into()))?;

        // 可见性包装在 `tweet` 下嵌套了真实的推文。
        let inner = result.get("tweet").unwrap_or(result);
        let tweet_id = inner
            .get("rest_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let legacy = inner.get("legacy").filter(|v| !v.is_null());
        let Some(legacy) = legacy else {
            if inner.get("__typename").and_then(Value::as_str) == Some("TweetTombstone") {
                return Err(ResolveError::Upstream(
                    "该推文开启了限制, 匿名用户无法查看".into(),
                ));
            }
            let reason = inner
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("无法读取该推文");
            return Err(ResolveError::Upstream(reason.to_string()));
        };

        // 优先使用长格式的笔记文本。
        let full_text = inner
            .pointer("/note_tweet/note_tweet_results/result/text")
            .and_then(Value::as_str)
            .or_else(|| legacy.get("full_text").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();

        let media = legacy
            .pointer("/entities/media")
            .and_then(Value::as_array)
            .map(|items| self.parse_media(&tweet_id, items))
            .unwrap_or_default();

        let text = strip_trailing_tco(&full_text, !media.is_empty());

        Ok(ResolvedPost {
            platform: "twitter".into(),
            source_url: source_url.to_string(),
            title: String::new(),
            text,
            media,
        })
    }

    fn parse_media(&self, tweet_id: &str, items: &[Value]) -> Vec<MediaItem> {
        let mut media = Vec::new();
        for item in items {
            let kind = match item.get("type").and_then(Value::as_str) {
                Some("photo") => MediaKind::Image,
                Some("video") => MediaKind::Video,
                Some("animated_gif") => MediaKind::Gif,
                _ => continue,
            };
            let media_url = item.get("media_url_https").and_then(Value::as_str);
            let width = item
                .pointer("/original_info/width")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let height = item
                .pointer("/original_info/height")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let index = media.len() + 1;

            let resolved = match kind {
                MediaKind::Image => {
                    let Some(base) = media_url else { continue };
                    let ext = url_ext(base, "jpg");
                    MediaItem {
                        kind,
                        url: with_image_name(base, "orig"),
                        width,
                        height,
                        duration_secs: None,
                        filename: format!("{tweet_id}_{index}.{ext}"),
                        ext,
                    }
                }
                MediaKind::Video | MediaKind::Gif => {
                    let variants = item
                        .pointer("/video_info/variants")
                        .and_then(Value::as_array);
                    let Some(url) = variants.and_then(|v| best_mp4(v)) else {
                        continue;
                    };
                    let duration = item
                        .pointer("/video_info/duration_millis")
                        .and_then(Value::as_u64)
                        .map(|ms| (ms / 1000) as u32);
                    MediaItem {
                        kind,
                        url,
                        width,
                        height,
                        duration_secs: duration,
                        ext: "mp4".into(),
                        filename: format!("{tweet_id}_{index}.mp4"),
                    }
                }
            };
            media.push(resolved);
        }
        media
    }
}

impl Default for TwitterResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MediaResolver for TwitterResolver {
    fn name(&self) -> &str {
        "twitter"
    }

    fn can_handle(&self, url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        lower.contains("/status/")
            && ["twitter.com", "//x.com", ".x.com", "fixupx.com"]
                .iter()
                .any(|host| lower.contains(host))
    }

    async fn resolve(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ResolvedPost, ResolveError> {
        let tweet_id = Self::tweet_id(url).ok_or(ResolveError::InvalidUrl)?;
        let body = self.fetch(client, &tweet_id).await?;
        let post = self.parse(url, &body)?;
        if post.media.is_empty() {
            return Err(ResolveError::NoMedia);
        }
        Ok(post)
    }
}

/// 将 `name=<size>` 追加到 `pbs.twimg.com` 图片 URL 以请求变体。
fn with_image_name(url: &str, size: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}name={size}")
}

/// 选择最高比特率的 `video/mp4` 变体 URL。
fn best_mp4(variants: &[Value]) -> Option<String> {
    variants
        .iter()
        .filter(|v| v.get("content_type").and_then(Value::as_str) == Some("video/mp4"))
        .max_by_key(|v| v.get("bit_rate").and_then(Value::as_u64).unwrap_or(0))
        .and_then(|v| v.get("url").and_then(Value::as_str))
        .map(str::to_owned)
}

/// 当推文携带媒体时（指向媒体本身而非真实内容），删除 Twitter 追加的尾部
/// `https://t.co/...` 短链接。
fn strip_trailing_tco(text: &str, has_media: bool) -> String {
    if !has_media {
        return text.to_string();
    }
    let trimmed = text.trim_end();
    if let Some(idx) = trimmed.rfind("https://t.co/") {
        let tail = &trimmed[idx..];
        if !tail[13..].contains(char::is_whitespace) {
            return trimmed[..idx].trim_end().to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tweet_id_from_various_hosts() {
        assert_eq!(
            TwitterResolver::tweet_id("https://x.com/jack/status/20"),
            Some("20".into())
        );
        assert_eq!(
            TwitterResolver::tweet_id("https://twitter.com/u/status/1733?s=20"),
            Some("1733".into())
        );
        assert_eq!(TwitterResolver::tweet_id("https://x.com/jack"), None);
    }

    #[test]
    fn can_handle_matches_status_links_only() {
        let r = TwitterResolver::new();
        assert!(r.can_handle("https://x.com/jack/status/20"));
        assert!(r.can_handle("https://twitter.com/jack/status/20"));
        assert!(r.can_handle("https://fixupx.com/jack/status/20"));
        assert!(!r.can_handle("https://x.com/jack"));
        assert!(!r.can_handle("https://example.com/status/20"));
    }

    #[test]
    fn image_url_and_ext_helpers() {
        assert_eq!(
            with_image_name("https://pbs.twimg.com/media/AbC.jpg", "orig"),
            "https://pbs.twimg.com/media/AbC.jpg?name=orig"
        );
        assert_eq!(
            url_ext("https://pbs.twimg.com/media/AbC.png?x=1", "jpg"),
            "png"
        );
        assert_eq!(url_ext("https://pbs.twimg.com/media/AbC", "jpg"), "jpg");
    }

    #[test]
    fn best_mp4_picks_highest_bitrate() {
        let variants = serde_json::json!([
            {"content_type": "application/x-mpegURL", "url": "a.m3u8"},
            {"content_type": "video/mp4", "bit_rate": 256000, "url": "low.mp4"},
            {"content_type": "video/mp4", "bit_rate": 2176000, "url": "high.mp4"},
        ]);
        assert_eq!(
            best_mp4(variants.as_array().unwrap()),
            Some("high.mp4".into())
        );
    }

    #[test]
    fn parses_photo_and_video_post() {
        let body = serde_json::json!({
            "data": {"tweetResult": {"result": {
                "rest_id": "123",
                "legacy": {
                    "full_text": "hello world https://t.co/abc",
                    "entities": {"media": [
                        {
                            "type": "photo",
                            "media_url_https": "https://pbs.twimg.com/media/P1.jpg",
                            "original_info": {"width": 1200, "height": 800}
                        },
                        {
                            "type": "video",
                            "media_url_https": "https://pbs.twimg.com/media/V1.jpg",
                            "original_info": {"width": 1280, "height": 720},
                            "video_info": {
                                "duration_millis": 30000,
                                "variants": [
                                    {"content_type": "video/mp4", "bit_rate": 832000, "url": "lo.mp4"},
                                    {"content_type": "video/mp4", "bit_rate": 2176000, "url": "hi.mp4"}
                                ]
                            }
                        }
                    ]}
                }
            }}}
        });
        let post = TwitterResolver::new()
            .parse("https://x.com/u/status/123", &body)
            .unwrap();
        assert_eq!(post.text, "hello world");
        assert_eq!(post.media.len(), 2);

        let photo = &post.media[0];
        assert_eq!(photo.kind, MediaKind::Image);
        assert_eq!(photo.url, "https://pbs.twimg.com/media/P1.jpg?name=orig");
        assert_eq!(photo.filename, "123_1.jpg");

        let video = &post.media[1];
        assert_eq!(video.kind, MediaKind::Video);
        assert_eq!(video.url, "hi.mp4");
        assert_eq!(video.duration_secs, Some(30));
        assert_eq!(video.filename, "123_2.mp4");
    }

    #[test]
    fn tombstone_is_reported() {
        let body = serde_json::json!({
            "data": {"tweetResult": {"result": {
                "__typename": "TweetTombstone"
            }}}
        });
        let err = TwitterResolver::new()
            .parse("https://x.com/u/status/1", &body)
            .unwrap_err();
        assert!(matches!(err, ResolveError::Upstream(_)));
    }
}
