//! URL probing. Port of the Python `downloader.probe` module.

use percent_encoding::percent_decode_str;
use rdm_domain::validation::sanitize_filename;
use reqwest::header::{
    HeaderMap, HeaderName, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG, LAST_MODIFIED,
    RANGE,
};
use url::Url;

use crate::error::HttpError;

/// What a probe learned about a download target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub final_url: String,
    pub filename: String,
    pub total_size: Option<u64>,
    pub supports_ranges: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A parsed HTTP `Content-Range` header.
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

/// Probe `url` to discover its size, range support and validators.
///
/// A best-effort `HEAD` collects headers some servers only expose there, then a
/// single-byte ranged `GET` settles the questions that matter: a `206` proves
/// range support, a `416` with `Content-Range: */0` means an empty file, and a
/// `200` falls back to `Content-Length`.
pub async fn probe_url(
    client: &reqwest::Client,
    url: &str,
    request_headers: &HeaderMap,
) -> Result<ProbeResult, HttpError> {
    let mut head_headers: Option<HeaderMap> = None;
    let mut final_url = url.to_string();

    // HEAD is advisory: many servers reject it, so any failure or error status
    // is ignored and the ranged GET below remains the source of truth.
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

    // Prefer the GET response header, falling back to whatever HEAD reported.
    let pick = |name: &HeaderName| -> Option<String> {
        header_str(&get_headers, name)
            .or_else(|| head_headers.as_ref().and_then(|h| header_str(h, name)))
    };
    let etag = pick(&ETAG);
    let last_modified = pick(&LAST_MODIFIED);
    let disposition = pick(&CONTENT_DISPOSITION);

    Ok(ProbeResult {
        filename: filename_from_headers(disposition.as_deref(), &final_url),
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

/// Extract the total length from a `Content-Range` value, or `None` when it is
/// absent, unknown (`*`) or unparseable.
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

/// Decide the output filename from `Content-Disposition`, falling back to the
/// URL path's last component, always percent-decoded and sanitized.
pub(crate) fn filename_from_headers(content_disposition: Option<&str>, url: &str) -> String {
    if let Some(disposition) = content_disposition {
        if let Some(name) = parse_content_disposition_filename(disposition) {
            if !name.is_empty() {
                return sanitize_filename(&name);
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
    sanitize_filename(name)
}

/// Parse a filename from a `Content-Disposition` header, preferring the RFC
/// 5987 `filename*` form over a plain `filename`.
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

/// Decode an RFC 5987 `charset'lang'percent-encoded` extended value.
fn decode_ext_value(raw: &str) -> Option<String> {
    let encoded = raw.splitn(3, '\'').nth(2)?;
    Some(percent_decode(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // The host may export HTTP_PROXY (e.g. a local VPN), which would route the
    // loopback mock requests through it; bypass it for these tests.
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
            filename_from_headers(None, "https://example.test/path/my%20file.bin"),
            "my file.bin"
        );
        assert_eq!(
            filename_from_headers(None, "https://example.test"),
            "download"
        );
    }

    #[test]
    fn filename_prefers_content_disposition() {
        assert_eq!(
            filename_from_headers(
                Some("attachment; filename=\"fixture.bin\""),
                "https://example.test/range.bin"
            ),
            "fixture.bin"
        );
        // RFC 5987 extended form wins over a plain filename.
        assert_eq!(
            filename_from_headers(
                Some("attachment; filename=\"x\"; filename*=UTF-8''na%C3%AFve.txt"),
                "https://example.test/range.bin"
            ),
            "naïve.txt"
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
