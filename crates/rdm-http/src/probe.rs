//! URL 探测。

use percent_encoding::percent_decode_str;
use rdm_domain::validation::sanitize_filename;
use reqwest::header::{
    HeaderMap, HeaderName, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
    LAST_MODIFIED, RANGE,
};
use url::Url;

use crate::error::HttpError;

/// 一次探测所获取到的下载目标信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub final_url: String,
    pub filename: String,
    pub total_size: Option<u64>,
    pub supports_ranges: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// 已解析的 HTTP `Content-Range` 响应头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentRange {
    Bytes {
        start: u64,
        end: u64,
        total: Option<u64>,
    },
    Unsatisfied {
        total: Option<u64>,
    },
}

/// 探测 `url`，获取其大小、Range 支持情况以及校验信息。
///
/// 先尽力发送 `HEAD` 收集某些服务器仅在响应头中暴露的元数据，
/// 再发送单字节范围 `GET` 来确定关键问题：`206` 证明支持 Range，
/// 携带 `Content-Range: */0` 的 `416` 表示空文件，
/// 而 `200` 则回退到 `Content-Length` 作为大小。
pub async fn probe_url(
    client: &reqwest::Client,
    url: &str,
    request_headers: &HeaderMap,
) -> Result<ProbeResult, HttpError> {
    let mut head_headers: Option<HeaderMap> = None;
    let mut final_url = url.to_string();

    // HEAD 只是参考：许多服务器会拒绝它，因此任何失败或错误状态
    // 都会被忽略，后续的 Range GET 仍作为真实来源。
    if let Ok(response) = client
        .head(url)
        .headers(request_headers.clone())
        .send()
        .await
    {
        if response.status().as_u16() < 400 {
            final_url = response.url().to_string();
            head_headers = Some(response.headers().clone());
        }
    }

    let response = client
        .get(&final_url)
        .headers(request_headers.clone())
        .header(RANGE, "bytes=0-0")
        .send()
        .await?;
    let status = response.status();
    let get_headers = response.headers().clone();
    final_url = response.url().to_string();
    drop(response);

    let range_size = size_from_content_range(header_str(&get_headers, &CONTENT_RANGE).as_deref());
    let empty_file = status.as_u16() == 416 && range_size == Some(0);
    if !empty_file && !status.is_success() {
        return Err(HttpError::Status {
            status: status.as_u16(),
            url: final_url,
        });
    }
    let supports_ranges = status.as_u16() == 206;

    let mut total_size = range_size;
    if total_size.is_none() && status.as_u16() == 200 {
        total_size = header_str(&get_headers, &CONTENT_LENGTH).and_then(|s| s.parse::<u64>().ok());
    }

    // 优先使用 GET 响应头，回退到 HEAD 报告的值。
    let pick = |name: &HeaderName| -> Option<String> {
        header_str(&get_headers, name)
            .or_else(|| head_headers.as_ref().and_then(|h| header_str(h, name)))
    };
    let etag = pick(&ETAG);
    let last_modified = pick(&LAST_MODIFIED);
    let disposition = pick(&CONTENT_DISPOSITION);
    let content_type = pick(&CONTENT_TYPE);

    Ok(ProbeResult {
        filename: filename_from_headers(
            disposition.as_deref(),
            &final_url,
            content_type.as_deref(),
        ),
        final_url,
        total_size,
        supports_ranges,
        etag,
        last_modified,
    })
}

fn header_str(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

fn percent_decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// 从 `Content-Range` 字符串中解析总长度；若该值缺失、
/// 未知（`*`）或无法解析则返回 `None`。
pub fn parse_content_range(value: &str) -> Option<ContentRange> {
    let (unit, range) = value.trim().split_once(char::is_whitespace)?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (bounds, total) = range.trim().split_once('/')?;
    let total = match total.trim() {
        "*" => None,
        value => Some(value.parse::<u64>().ok()?),
    };
    if bounds.trim() == "*" {
        return Some(ContentRange::Unsatisfied { total });
    }
    let (start, end) = bounds.trim().split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    if end < start || total.is_some_and(|total| end >= total) {
        return None;
    }
    Some(ContentRange::Bytes { start, end, total })
}

pub(crate) fn size_from_content_range(value: Option<&str>) -> Option<u64> {
    match parse_content_range(value?)? {
        ContentRange::Bytes { total, .. } | ContentRange::Unsatisfied { total } => total,
    }
}

fn extension_from_content_type(content_type: Option<&str>) -> Option<&'static str> {
    let mime = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "image/tiff" => "tif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/json" => "json",
        _ => return None,
    })
}

fn has_extension(filename: &str) -> bool {
    let Some((_, ext)) = filename.rsplit_once('.') else {
        return false;
    };
    !ext.is_empty()
        && ext.len() <= 12
        && ext.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn append_extension_if_missing(filename: String, content_type: Option<&str>) -> String {
    if has_extension(&filename) {
        return filename;
    }
    let Some(ext) = extension_from_content_type(content_type) else {
        return filename;
    };
    let suffix = format!(".{ext}");
    let max_base = 240usize.saturating_sub(suffix.chars().count());
    let base = filename.chars().take(max_base).collect::<String>();
    format!("{base}{suffix}")
}

/// 根据 `Content-Disposition` 决定输出文件名，若无则回退到 URL 路径
/// 的最后一段；如果文件名无扩展名，使用 `Content-Type` 补全常见类型。
/// 最终结果都会进行百分号解码与 Windows 安全的清理。
pub(crate) fn filename_from_headers(
    content_disposition: Option<&str>,
    url: &str,
    content_type: Option<&str>,
) -> String {
    if let Some(disposition) = content_disposition {
        if let Some(name) = parse_content_disposition_filename(disposition) {
            if !name.is_empty() {
                return append_extension_if_missing(sanitize_filename(&name), content_type);
            }
        }
    }
    let path = Url::parse(url)
        .map(|u| u.path().to_string())
        .unwrap_or_default();
    let decoded = percent_decode(&path);
    let base = decoded
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("");
    let name = if base.is_empty() { "download" } else { base };
    append_extension_if_missing(sanitize_filename(name), content_type)
}

/// 从 `Content-Disposition` 头中解析文件名，优先使用 RFC 5987
/// 的 `filename*` 扩展形式，其次是普通的 `filename`。
fn parse_content_disposition_filename(value: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    for part in value.split(';') {
        let Some((key, raw)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let raw = raw.trim();
        if key == "filename*" {
            extended = decode_ext_value(raw);
        } else if key == "filename" {
            plain = Some(percent_decode(raw.trim_matches('"')));
        }
    }
    extended.or(plain)
}

/// 解码 RFC 5987 `charset'lang'percent-encoded` 形式的扩展值。
fn decode_ext_value(raw: &str) -> Option<String> {
    let encoded = raw.splitn(3, '\'').nth(2)?;
    Some(percent_decode(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // 主机可能暴露了 HTTP_PROXY（例如本地 VPN），这会将回环测试请求
    // 也路由过去；在这些测试中绕过它。
    fn test_client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[test]
    fn content_range_size_rejects_unknown_and_invalid_values() {
        assert_eq!(size_from_content_range(None), None);
        assert_eq!(size_from_content_range(Some("invalid")), None);
        assert_eq!(size_from_content_range(Some("bytes 0-0/*")), None);
        assert_eq!(
            size_from_content_range(Some("bytes 0-0/not-a-number")),
            None
        );
        assert_eq!(
            size_from_content_range(Some("bytes 0-0/12345")),
            Some(12345)
        );
    }

    #[test]
    fn parses_satisfied_and_unsatisfied_content_ranges() {
        assert_eq!(
            parse_content_range("bytes 10-19/100"),
            Some(ContentRange::Bytes {
                start: 10,
                end: 19,
                total: Some(100),
            })
        );
        assert_eq!(
            parse_content_range("BYTES */0"),
            Some(ContentRange::Unsatisfied { total: Some(0) })
        );
        assert_eq!(parse_content_range("items 0-1/2"), None);
        assert_eq!(parse_content_range("bytes 20-10/100"), None);
        assert_eq!(parse_content_range("bytes 0-100/100"), None);
    }

    #[test]
    fn filename_falls_back_to_url_path() {
        assert_eq!(
            filename_from_headers(None, "https://example.test/path/my%20file.bin", None),
            "my file.bin"
        );
        assert_eq!(
            filename_from_headers(None, "https://example.test", None),
            "download"
        );
    }

    #[test]
    fn filename_uses_content_type_when_url_has_no_extension() {
        assert_eq!(
            filename_from_headers(
                None,
                "https://img1.baidu.com/it/u=910271416,2899451860&fm=253&fmt=auto&app=138&f=JPEG?w=751&h=500",
                Some("image/jpeg; charset=binary")
            ),
            "u=910271416,2899451860&fm=253&fmt=auto&app=138&f=JPEG.jpg"
        );
        assert_eq!(
            filename_from_headers(None, "https://example.test/image", Some("image/png")),
            "image.png"
        );
        assert_eq!(
            filename_from_headers(None, "https://example.test/image.jpg", Some("image/png")),
            "image.jpg"
        );
    }

    #[test]
    fn filename_prefers_content_disposition() {
        assert_eq!(
            filename_from_headers(
                Some("attachment; filename=\"fixture.bin\""),
                "https://example.test/range.bin",
                None
            ),
            "fixture.bin"
        );
        // RFC 5987 扩展形式优先于普通文件名。
        assert_eq!(
            filename_from_headers(
                Some("attachment; filename=\"x\"; filename*=UTF-8''na%C3%AFve.txt"),
                "https://example.test/range.bin",
                None
            ),
            "naïve.txt"
        );
        assert_eq!(
            filename_from_headers(
                Some("attachment; filename=\"download\""),
                "https://example.test/range",
                Some("application/pdf")
            ),
            "download.pdf"
        );
    }

    #[tokio::test]
    async fn probe_detects_range_metadata() {
        let server = MockServer::start().await;
        let total: u64 = 4096;
        Mock::given(path("/range.bin"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", format!("bytes 0-0/{total}").as_str())
                    .insert_header("ETag", "\"v1\"")
                    .insert_header("Accept-Ranges", "bytes")
                    .insert_header(
                        "Content-Disposition",
                        "attachment; filename=\"fixture.bin\"",
                    )
                    .set_body_bytes(vec![0u8]),
            )
            .mount(&server)
            .await;

        let client = test_client();
        let result = probe_url(
            &client,
            &format!("{}/range.bin", server.uri()),
            &HeaderMap::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.filename, "fixture.bin");
        assert_eq!(result.total_size, Some(total));
        assert!(result.supports_ranges);
        assert_eq!(result.etag.as_deref(), Some("\"v1\""));
    }

    #[tokio::test]
    async fn probe_falls_back_when_range_is_ignored() {
        let server = MockServer::start().await;
        let body = vec![7u8; 2048];
        Mock::given(path("/no-range.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let client = test_client();
        let result = probe_url(
            &client,
            &format!("{}/no-range.bin", server.uri()),
            &HeaderMap::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.total_size, Some(body.len() as u64));
        assert!(!result.supports_ranges);
    }

    #[tokio::test]
    async fn probe_accepts_empty_file() {
        let server = MockServer::start().await;
        Mock::given(path("/empty.bin"))
            .respond_with(ResponseTemplate::new(416).insert_header("Content-Range", "bytes */0"))
            .mount(&server)
            .await;

        let client = test_client();
        let result = probe_url(
            &client,
            &format!("{}/empty.bin", server.uri()),
            &HeaderMap::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.total_size, Some(0));
        assert!(!result.supports_ranges);
    }

    #[tokio::test]
    async fn probe_forwards_provider_headers() {
        let server = MockServer::start().await;
        Mock::given(path("/private.bin"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", "bytes 0-0/16")
                    .set_body_bytes(vec![0u8]),
            )
            .mount(&server)
            .await;
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-token".parse().unwrap());

        let result = probe_url(
            &test_client(),
            &format!("{}/private.bin", server.uri()),
            &headers,
        )
        .await
        .unwrap();

        assert_eq!(result.total_size, Some(16));
        assert!(result.supports_ranges);
    }
}
