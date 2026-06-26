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

/// 显示并聚焦主窗口，使确认对话框立刻可见。
fn show_window(app: &AppHandle) {
    use crate::show_main_window;
    show_main_window(app);
}

/// OPTIONS 预检直接返回 204，CORS 头由 [`cors_layer`] 统一注入。
async fn no_content() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// 为每个响应注入 CORS 头，使浏览器扩展的 service worker 能跨域访问本桥。
/// 不引入 tower-http，保持依赖精简。
async fn cors_layer(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("access-control-allow-origin"),
        HeaderValue::from_static("*"),
    );
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
}
