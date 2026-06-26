//! 浏览器扩展与桌面端之间的本地 HTTP 桥（仅监听 127.0.0.1）。
//!
//! 由 [`crate::run`] 的 Tauri setup hook 在 tokio 运行时内 spawn。
//! 浏览器扩展拦截到下载后向本桥 POST 下载 URL；本桥校验通过后，
//! 通过 `rdm://external-download` 事件把请求交给前端，弹出与
//! 「新建下载」相同的确认对话框（含文件名/保存位置/连接数），
//! 用户确认后才开始下载——体验对齐 IDM 的拦截确认流程。

use std::net::SocketAddr;

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use rdm_domain::validation::is_http_url;

/// 浏览器扩展连接的固定端口。桌面端与扩展各自定义并保持一致；
/// v1 不做可配置，以减少改动面。
pub const BRIDGE_PORT: u16 = 43721;

/// 前端事件名：把外部下载请求交给 AddDialog 确认。
pub const EXTERNAL_DOWNLOAD_EVENT: &str = "rdm://external-download";

/// 前端事件名：把一批嗅探到的媒体候选交给批量确认对话框。
pub const SNIFFED_MEDIA_EVENT: &str = "rdm://sniffed-media";

/// `/media-candidates` 一次最多接收的有效候选数（防滥用/防误塞）。
const MAX_CANDIDATES: usize = 100;
/// 单个候选 URL 的最大长度（字节）。超过视为非法，跳过。
const MAX_URL_LEN: usize = 4096;
/// 文件名最大长度（字符）。超过则丢弃文件名，交由下载引擎从 URL 推断。
const MAX_FILENAME_LEN: usize = 255;
/// kind/ext 字段的最大长度（字符），超长按缺省（None）处理。
const MAX_SHORT_FIELD_LEN: usize = 32;
/// 页面标题最大长度（字符），超长截断（仅用于展示）。
const MAX_PAGE_TITLE_LEN: usize = 200;

/// `POST /downloads` 的请求体。`url`/`filename` 用于预填确认框，
/// 其余字段仅为兼容现有请求体形状而接收（v1 不预填）。
#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub destination: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub connections: Option<u32>,
    #[serde(default)]
    #[allow(dead_code)]
    pub sha256: Option<String>,
}

/// 推送给前端的事件载荷，预填 AddDialog。
#[derive(Debug, Clone, Serialize)]
pub struct ExternalDownload {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// `POST /media-candidates` 的请求体：浏览器扩展嗅探到的一批媒体候选。
/// 字段为驼峰命名以对齐扩展侧的 MediaCandidate 形状。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCandidatesRequest {
    #[serde(default)]
    pub candidates: Vec<MediaCandidateInput>,
    #[serde(default)]
    pub page_title: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub page_url: Option<String>,
}

/// 单个媒体候选输入。仅接收用于批量确认/下载所需的字段。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCandidateInput {
    pub url: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub ext: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub bytes: Option<u64>,
}

/// 推送给前端的批量确认载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SniffedMedia {
    pub candidates: Vec<SniffedCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_title: Option<String>,
}

/// 校验后的单个候选（确保 url 为合法 http/https）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SniffedCandidate {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// `GET /health` 的响应。
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

/// 成功接收下载请求时的响应。
#[derive(Debug, Serialize)]
struct AcceptedResponse {
    status: &'static str,
}

/// 错误响应。
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// 启动本地 HTTP 桥。绑定失败仅记录日志并返回——下载管理器与
/// 桌面 UI 仍可正常工作，只是浏览器扩展无法连入。
pub async fn serve(app: AppHandle) {
    let router = Router::new()
        .route("/health", get(health))
        .route(
            "/downloads",
            post(create_download_handler).options(no_content),
        )
        .route(
            "/media-candidates",
            post(create_media_candidates_handler).options(no_content),
        )
        .layer(middleware::from_fn(cors_layer))
        .with_state(app);

    let addr = SocketAddr::from(([127, 0, 0, 1], BRIDGE_PORT));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            log::error!("Failed to bind browser bridge on {addr}: {error}");
            return;
        }
    };
    log::info!("RDM browser bridge listening on http://{addr}");
    if let Err(error) = axum::serve(listener, router).await {
        log::error!("Browser bridge stopped: {error}");
    }
}

/// 校验 `req`，成功则返回要预填进确认框的外部下载载荷。
/// 拆成纯函数以便单测；不在此时创建任务。
fn validate_request(req: &DownloadRequest) -> Result<ExternalDownload, ErrorResponse> {
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return Err(ErrorResponse {
            error: "缺少 url。".to_string(),
        });
    }
    if !is_http_url(&url) {
        return Err(ErrorResponse {
            error: "url 必须是合法的 http/https 链接。".to_string(),
        });
    }
    let filename = req
        .filename
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    Ok(ExternalDownload { url, filename })
}

/// 去除首尾空白；为空或超过 `max` 字符则返回 None。
fn clean_field(s: &Option<String>, max: usize) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.chars().count() <= max)
        .map(str::to_string)
}

/// 校验批量候选并施加防护上限：
/// - 过滤非 http/https、空 URL、超长 URL（>`MAX_URL_LEN`）；
/// - 文件名/kind/ext 超长按缺省处理（不截断文件名以免破坏扩展名）；
/// - 最多保留 `MAX_CANDIDATES` 个有效候选；页面标题截断到 `MAX_PAGE_TITLE_LEN`。
/// 全部无效时返回错误。拆成纯函数以便单测；不在此时创建任务。
fn validate_media_candidates(req: &MediaCandidatesRequest) -> Result<SniffedMedia, ErrorResponse> {
    let mut candidates = Vec::new();
    for c in &req.candidates {
        if candidates.len() >= MAX_CANDIDATES {
            break; // 超量：只取前 MAX_CANDIDATES 个有效候选
        }
        let url = c.url.trim().to_string();
        if url.is_empty() || url.len() > MAX_URL_LEN || !is_http_url(&url) {
            continue; // 跳过非法/非 http/超长候选
        }
        candidates.push(SniffedCandidate {
            url,
            filename: clean_field(&c.filename, MAX_FILENAME_LEN),
            kind: clean_field(&c.kind, MAX_SHORT_FIELD_LEN),
            ext: clean_field(&c.ext, MAX_SHORT_FIELD_LEN),
            width: c.width,
            height: c.height,
            duration: c.duration,
            bytes: c.bytes,
        });
    }

    if candidates.is_empty() {
        return Err(ErrorResponse {
            error: "没有有效的媒体候选。".to_string(),
        });
    }

    let page_title = req
        .page_title
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(MAX_PAGE_TITLE_LEN).collect::<String>());

    Ok(SniffedMedia {
        candidates,
        page_title,
    })
}

async fn health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn create_download_handler(
    State(app): State<AppHandle>,
    Json(req): Json<DownloadRequest>,
) -> Response {
    match validate_request(&req) {
        Ok(payload) => {
            // 把请求交给前端：弹出与新建下载相同的确认对话框，
            // 用户点「开始下载」后才真正创建任务。
            if let Err(error) = app.emit(EXTERNAL_DOWNLOAD_EVENT, &payload) {
                log::error!("Failed to emit {EXTERNAL_DOWNLOAD_EVENT}: {error}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: error.to_string(),
                    }),
                )
                    .into_response();
            }
            show_window(&app);
            (
                StatusCode::ACCEPTED,
                Json(AcceptedResponse {
                    status: "accepted; awaiting user confirmation",
                }),
            )
                .into_response()
        }
        Err(body) => (StatusCode::BAD_REQUEST, Json(body)).into_response(),
    }
}

async fn create_media_candidates_handler(
    State(app): State<AppHandle>,
    Json(req): Json<MediaCandidatesRequest>,
) -> Response {
    match validate_media_candidates(&req) {
        Ok(payload) => {
            // 把整批候选交给前端：弹出批量确认对话框，用户勾选后才创建任务。
            if let Err(error) = app.emit(SNIFFED_MEDIA_EVENT, &payload) {
                log::error!("Failed to emit {SNIFFED_MEDIA_EVENT}: {error}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: error.to_string(),
                    }),
                )
                    .into_response();
            }
            show_window(&app);
            (
                StatusCode::ACCEPTED,
                Json(AcceptedResponse {
                    status: "accepted; awaiting user confirmation",
                }),
            )
                .into_response()
        }
        Err(body) => (StatusCode::BAD_REQUEST, Json(body)).into_response(),
    }
}

/// 显示并聚焦主窗口，使确认对话框立刻可见。
fn show_window(app: &AppHandle) {
    use crate::show_main_window;
    show_main_window(app);
}

/// OPTIONS 预检直接返回 204，CORS 头由 [`cors_layer`] 统一注入。
async fn no_content() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// 是否为浏览器扩展来源（Chrome/Firefox/Safari 扩展）。
fn is_extension_origin(origin: &str) -> bool {
    origin.starts_with("chrome-extension://")
        || origin.starts_with("moz-extension://")
        || origin.starts_with("safari-web-extension://")
}

/// 校验来源并注入 CORS 头。
///
/// 只服务于浏览器扩展或无 `Origin` 的本地调用（扩展 service worker 常不带
/// Origin）；带网页 `Origin`（http/https）的请求一律 403——含 OPTIONS 预检，
/// 因此连「简单请求」也会被挡在副作用之前。这样任意网站即便 fetch 到本地桥，
/// 也无法借此触发下载/弹窗。不引入 tower-http，保持依赖精简。
async fn cors_layer(req: Request, next: Next) -> Response {
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    if let Some(ref o) = origin {
        if !is_extension_origin(o) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    // 走到这里的 Origin 必为扩展来源：回显它（不再用通配 `*`）。
    if let Some(o) = origin {
        if let Ok(value) = HeaderValue::from_str(&o) {
            headers.insert(
                HeaderName::from_static("access-control-allow-origin"),
                value,
            );
            headers.insert(HeaderName::from_static("vary"), HeaderValue::from_static("Origin"));
        }
    }
    headers.insert(
        HeaderName::from_static("access-control-allow-headers"),
        HeaderValue::from_static("Content-Type"),
    );
    headers.insert(
        HeaderName::from_static("access-control-allow-methods"),
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str) -> DownloadRequest {
        DownloadRequest {
            url: url.to_string(),
            filename: None,
            destination: None,
            connections: None,
            sha256: None,
        }
    }

    #[test]
    fn validate_accepts_http_url() {
        let payload = validate_request(&req("https://example.com/file.zip")).unwrap();
        assert_eq!(payload.url, "https://example.com/file.zip");
        assert!(payload.filename.is_none());
    }

    #[test]
    fn validate_preserves_filename() {
        let request = DownloadRequest {
            url: "https://example.com/video.mp4".to_string(),
            filename: Some("  movie.mp4  ".to_string()),
            destination: None,
            connections: None,
            sha256: None,
        };
        let payload = validate_request(&request).unwrap();
        assert_eq!(payload.filename.as_deref(), Some("movie.mp4"));
    }

    #[test]
    fn validate_drops_blank_filename() {
        let request = DownloadRequest {
            url: "https://example.com/x.bin".to_string(),
            filename: Some("   ".to_string()),
            destination: None,
            connections: None,
            sha256: None,
        };
        let payload = validate_request(&request).unwrap();
        assert!(payload.filename.is_none());
    }

    #[test]
    fn validate_rejects_missing_url() {
        let body = validate_request(&req("   ")).unwrap_err();
        assert!(!body.error.is_empty());
    }

    #[test]
    fn validate_rejects_non_http_url() {
        assert!(validate_request(&req("ftp://example.com/x")).is_err());
        assert!(validate_request(&req("not a url")).is_err());
    }

    /// 编译期断言：`ExternalDownload` 能被序列化为前端可消费的事件载荷。
    #[test]
    fn external_download_serializes() {
        let payload = ExternalDownload {
            url: "https://example.com/a".to_string(),
            filename: Some("a.zip".to_string()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["url"], "https://example.com/a");
        assert_eq!(json["filename"], "a.zip");

        let no_name = ExternalDownload {
            url: "https://example.com/b".to_string(),
            filename: None,
        };
        let json = serde_json::to_value(&no_name).unwrap();
        assert!(json.get("filename").is_none());
    }

    fn candidate(url: &str) -> MediaCandidateInput {
        MediaCandidateInput {
            url: url.to_string(),
            filename: None,
            kind: None,
            ext: None,
            width: None,
            height: None,
            duration: None,
            bytes: None,
        }
    }

    fn candidates_req(urls: &[&str]) -> MediaCandidatesRequest {
        MediaCandidatesRequest {
            candidates: urls.iter().map(|u| candidate(u)).collect(),
            page_title: None,
            page_url: None,
        }
    }

    #[test]
    fn media_candidates_keeps_only_http_urls() {
        let req = candidates_req(&[
            "https://example.com/a.mp4",
            "ftp://example.com/b.mp4",
            "blob:https://example.com/c",
            "   ",
            "http://example.com/d.jpg",
        ]);
        let payload = validate_media_candidates(&req).unwrap();
        let urls: Vec<_> = payload.candidates.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls, ["https://example.com/a.mp4", "http://example.com/d.jpg"]);
    }

    #[test]
    fn media_candidates_rejects_when_all_invalid() {
        let req = candidates_req(&["javascript:void(0)", "not a url"]);
        let body = validate_media_candidates(&req).unwrap_err();
        assert!(!body.error.is_empty());

        let empty = candidates_req(&[]);
        assert!(validate_media_candidates(&empty).is_err());
    }

    #[test]
    fn media_candidates_trims_blank_fields_to_none() {
        let mut req = candidates_req(&["https://example.com/v.mp4"]);
        req.candidates[0].filename = Some("  movie.mp4  ".to_string());
        req.candidates[0].kind = Some("   ".to_string());
        req.candidates[0].ext = Some("mp4".to_string());
        req.candidates[0].width = Some(1920);
        req.candidates[0].height = Some(1080);
        req.page_title = Some("  标题  ".to_string());

        let payload = validate_media_candidates(&req).unwrap();
        let c = &payload.candidates[0];
        assert_eq!(c.filename.as_deref(), Some("movie.mp4"));
        assert!(c.kind.is_none());
        assert_eq!(c.ext.as_deref(), Some("mp4"));
        assert_eq!(c.width, Some(1920));
        assert_eq!(payload.page_title.as_deref(), Some("标题"));
    }

    #[test]
    fn sniffed_media_serializes_camel_case() {
        let payload = SniffedMedia {
            candidates: vec![SniffedCandidate {
                url: "https://example.com/a.mp4".to_string(),
                filename: Some("a.mp4".to_string()),
                kind: Some("video".to_string()),
                ext: Some("mp4".to_string()),
                width: Some(1280),
                height: Some(720),
                duration: Some(12.5),
                bytes: Some(4096),
            }],
            page_title: Some("T".to_string()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["pageTitle"], "T");
        assert_eq!(json["candidates"][0]["url"], "https://example.com/a.mp4");
        assert_eq!(json["candidates"][0]["width"], 1280);

        // 可选字段为 None 时不应出现在序列化结果中。
        let minimal = SniffedMedia {
            candidates: vec![SniffedCandidate {
                url: "https://example.com/b".to_string(),
                filename: None,
                kind: None,
                ext: None,
                width: None,
                height: None,
                duration: None,
                bytes: None,
            }],
            page_title: None,
        };
        let json = serde_json::to_value(&minimal).unwrap();
        assert!(json.get("pageTitle").is_none());
        assert!(json["candidates"][0].get("width").is_none());
    }

    #[test]
    fn media_candidates_caps_count_at_max() {
        let urls: Vec<String> = (0..150).map(|i| format!("https://x.com/{i}.mp4")).collect();
        let req = MediaCandidatesRequest {
            candidates: urls.iter().map(|u| candidate(u)).collect(),
            page_title: None,
            page_url: None,
        };
        let payload = validate_media_candidates(&req).unwrap();
        assert_eq!(payload.candidates.len(), MAX_CANDIDATES);
    }

    #[test]
    fn media_candidates_skips_overlong_url() {
        let long = format!("https://x.com/{}.mp4", "a".repeat(MAX_URL_LEN));
        let req = MediaCandidatesRequest {
            candidates: vec![candidate(&long), candidate("https://x.com/ok.mp4")],
            page_title: None,
            page_url: None,
        };
        let payload = validate_media_candidates(&req).unwrap();
        let urls: Vec<_> = payload.candidates.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls, ["https://x.com/ok.mp4"]);
    }

    #[test]
    fn media_candidates_drops_overlong_filename_keeps_item() {
        let mut req = candidates_req(&["https://x.com/v.mp4"]);
        req.candidates[0].filename = Some("a".repeat(MAX_FILENAME_LEN + 1));
        let payload = validate_media_candidates(&req).unwrap();
        assert_eq!(payload.candidates.len(), 1);
        assert!(payload.candidates[0].filename.is_none());
    }

    #[test]
    fn media_candidates_truncates_page_title() {
        let mut req = candidates_req(&["https://x.com/v.mp4"]);
        req.page_title = Some("x".repeat(500));
        let payload = validate_media_candidates(&req).unwrap();
        assert_eq!(
            payload.page_title.as_deref().map(|s| s.chars().count()),
            Some(MAX_PAGE_TITLE_LEN),
        );
    }

    #[test]
    fn extension_origin_recognized_web_origin_rejected() {
        assert!(is_extension_origin("chrome-extension://abcdefghijklmnop"));
        assert!(is_extension_origin("moz-extension://uuid-here"));
        assert!(is_extension_origin("safari-web-extension://abc"));
        assert!(!is_extension_origin("https://evil.com"));
        assert!(!is_extension_origin("http://127.0.0.1:43721"));
        assert!(!is_extension_origin(""));
        assert!(!is_extension_origin("null"));
    }
}
