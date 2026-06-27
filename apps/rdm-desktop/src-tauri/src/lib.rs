//! RDM 桌面应用的 Tauri 后端。
//!
//! 基于 [`rdm_service::DownloadManager`] 的命令层，以及桌面集成功能：
//! 打开任务文件夹。前端订阅 `task://update` 获取实时进度，
//! 订阅 `rdm://open-url` / `rdm://new-download` 响应托盘/第二实例触发。
//!
//! [`bridge`] 模块提供一个仅监听 127.0.0.1 的本地 HTTP 桥，
//! 供浏览器扩展把拦截到的下载交给 RDM。

mod bridge;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

use rdm_domain::config::{AppSettings, SettingsStore};
use rdm_domain::validation::is_http_url;
use rdm_domain::DownloadTask;
use rdm_http::ProviderRegistry;
use rdm_resolver::{ProxyConfig, ResolvedPost, ResolverRegistry};
use rdm_service::DownloadManager;
use rdm_storage::DownloadDatabase;

/// Tauri 管理的共享应用状态。
struct AppState {
    manager: DownloadManager,
    settings_store: SettingsStore,
    /// 启动时传递给首个进程的 URL。前端在注册事件监听器后获取，
    /// 避免启动竞态。
    launch_url: Mutex<Option<String>>,
    /// 将社交媒体/网页帖子 URL 解析为可下载的媒体项。
    /// 使用互斥锁保护，以便在代理设置变更时可以重建而无需重启应用。
    resolvers: Mutex<Arc<ResolverRegistry>>,
    /// 用户真正想要退出时设置，让关闭处理程序退出而非隐藏到托盘。
    force_quit: AtomicBool,
}

/// 将 [`AppSettings`] 的代理部分转换为解析器的 [`ProxyConfig`]；
/// 代理禁用时返回不激活的配置。
fn proxy_from_settings(settings: &AppSettings) -> ProxyConfig {
    if settings.proxy_enabled {
        ProxyConfig {
            url: settings.proxy_url.clone(),
            username: settings.proxy_username.clone(),
            password: settings.proxy_password.clone(),
        }
    } else {
        ProxyConfig::default()
    }
}

/// 根据 `settings` 重建解析器注册表，替换状态中的现有实例。
/// 失败时仅记录日志而不向上传播：损坏的注册表仅意味着媒体解析功能
/// 不可用，但下载功能仍可正常工作。
fn rebuild_resolvers(state: &AppState, settings: &AppSettings) {
    match ResolverRegistry::new(proxy_from_settings(settings)) {
        Ok(registry) => {
            *state.resolvers.lock().unwrap() = Arc::new(registry);
        }
        Err(error) => log::error!("Failed to rebuild media resolvers: {error}"),
    }
}

/// 每次进度更新时推送到前端的数据载荷。
#[derive(Serialize, Clone)]
struct ProgressPayload {
    task: DownloadTask,
    speed: f64,
}

#[tauri::command]
fn list_tasks(state: State<'_, AppState>) -> Vec<DownloadTask> {
    state.manager.all_tasks()
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.manager.settings()
}

#[tauri::command]
fn take_launch_url(state: State<'_, AppState>) -> Option<String> {
    state.launch_url.lock().unwrap().take()
}

#[tauri::command]
fn add_download(
    state: State<'_, AppState>,
    url: String,
    destination: Option<String>,
    connections: Option<u32>,
    filename: Option<String>,
    sha256: Option<String>,
    referrer: Option<String>,
) -> Result<DownloadTask, String> {
    let destination = destination
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| state.manager.settings().download_dir);
    state
        .manager
        .add_download_with_referrer(
            &url,
            std::path::Path::new(&destination),
            connections,
            filename.as_deref().unwrap_or_default(),
            sha256.as_deref().unwrap_or_default(),
            referrer.as_deref().unwrap_or_default(),
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn start_task(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.start(&id).map_err(|error| error.to_string())
}

#[tauri::command]
fn pause_task(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.pause(&id).map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_task(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.cancel(&id).map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_task(state: State<'_, AppState>, id: String, delete_file: bool) -> Result<bool, String> {
    state
        .manager
        .delete(&id, delete_file)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: AppSettings) -> Result<AppSettings, String> {
    // 解析器客户端仅构建一次并由互斥锁保护；仅在代理实际发生变化时才重建，
    // 因为下载会实时读取设置。
    let old_proxy = proxy_from_settings(&state.manager.settings());
    let validated = state
        .settings_store
        .save(&settings)
        .map_err(|error| error.to_string())?;
    state.manager.update_settings(validated.clone());
    if proxy_from_settings(&validated) != old_proxy {
        rebuild_resolvers(&state, &validated);
    }
    Ok(validated)
}

/// 将社交媒体/网页帖子 URL 解析为可下载的媒体项。
///
/// 注册表在首次 `.await` 前从状态中克隆，使命令 future 不会
/// 跨越挂起点借用 `State`。
#[tauri::command]
async fn resolve_media(state: State<'_, AppState>, url: String) -> Result<ResolvedPost, String> {
    let resolvers = state.resolvers.lock().unwrap().clone();
    resolvers
        .resolve(url.trim())
        .await
        .map_err(|error| error.to_string())
}

/// 显示任务的输出文件，若文件名未知或文件尚不存在则回退到目标目录。
#[tauri::command]
fn reveal_task_file(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let task = state
        .manager
        .get_task(&id)
        .ok_or_else(|| "下载任务不存在。".to_string())?;
    let directory = std::path::PathBuf::from(&task.destination);
    let output = task.output_path();

    if output.is_file() {
        app.opener()
            .reveal_item_in_dir(output)
            .map_err(|error| error.to_string())
    } else {
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        app.opener()
            .open_path(directory.to_string_lossy(), None::<&str>)
            .map_err(|error| error.to_string())
    }
}

/// CLI 参数中的首个 http/https URL。
fn first_http_url(args: &[String]) -> Option<String> {
    args.iter().find(|arg| is_http_url(arg)).cloned()
}

/// 显示并聚焦主窗口。
pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 开始优雅退出：暂停/持久化进行中的下载，然后退出。
fn quit_app(app: &AppHandle) {
    let manager = {
        let state = app.state::<AppState>();
        state.force_quit.store(true, Ordering::SeqCst);
        state.manager.clone()
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        log::info!("Shutting down: persisting in-flight downloads");
        manager.shutdown().await;
        app.exit(0);
    });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let new = MenuItem::with_id(app, "new", "新建下载", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &new, &separator, &quit])?;

    let mut builder = TrayIconBuilder::with_id("rdm-tray")
        .tooltip("RDM 下载管理器")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "new" => {
                show_main_window(app);
                let _ = app.emit("rdm://new-download", ());
            }
            "quit" => quit_app(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 构建并运行 Tauri 应用。
pub fn run() {
    let mut builder = tauri::Builder::default();

    // 单实例必须先注册：第二次启动会将 URL 传递给运行中的实例
    //（该实例会显示窗口并打开添加对话框），然后退出。
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            show_main_window(app);
            if let Some(url) = first_http_url(&argv) {
                let _ = app.emit("rdm://open-url", url);
            }
        }));
    }

    builder
        // 应用日志目录中的轮转日志文件：当 `rdm.log` 超过大小上限时会轮转，
        // 并仅保留最近的备份，因此磁盘使用保持受限。同时镜像到 stdout
        // 以便 `app:dev` 使用。
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("rdm".into()),
                    }),
                ])
                .level(log::LevelFilter::Info)
                .max_file_size(2_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let database = Arc::new(DownloadDatabase::open(None)?);
            let settings_store = SettingsStore::new(None);
            let settings = settings_store.load();
            let launch_url = first_http_url(&std::env::args().collect::<Vec<_>>());

            // DownloadManager 通过 tokio::spawn 启动其调度器，因此必须在运行时
            // 上下文内构造；Tauri 的异步运行时是 tokio，生成的调度器存活时间
            // 超出此块。
            // 打包随附的 ffmpeg（资源目录）；开发期可能不存在，管理器会自动
            // 回退到 PATH 上的 ffmpeg，二者皆不可用时保留裸流。
            let ffmpeg_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
            let ffmpeg_candidate = app
                .path()
                .resolve(ffmpeg_name, tauri::path::BaseDirectory::Resource)
                .ok();
            let manager = tauri::async_runtime::block_on(async {
                DownloadManager::new(
                    database,
                    settings.clone(),
                    Arc::new(ProviderRegistry::default()),
                )
                .map(|manager| manager.with_ffmpeg(ffmpeg_candidate))
            })?;

            let emitter = app.handle().clone();
            manager.add_listener(Arc::new(move |task, speed| {
                let _ = emitter.emit("task://update", ProgressPayload { task, speed });
            }));

            let resolvers = Arc::new(ResolverRegistry::new(proxy_from_settings(&settings))?);

            app.manage(AppState {
                manager,
                settings_store,
                launch_url: Mutex::new(launch_url),
                resolvers: Mutex::new(resolvers),
                force_quit: AtomicBool::new(false),
            });

            log::info!(
                "RDM started; {} task(s) restored from database",
                app.state::<AppState>().manager.all_tasks().len()
            );

            // 启动浏览器扩展用的本地 HTTP 桥（仅 127.0.0.1）。Tauri 的
            // 异步运行时即 tokio，axum 可在此正常运行；绑定失败仅记录日志。
            tauri::async_runtime::spawn(bridge::serve(app.handle().clone()));

            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                let minimize = state.manager.settings().minimize_to_tray;
                if minimize && !state.force_quit.load(Ordering::SeqCst) {
                    // 在后台继续下载。
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    // 仅在引擎结束后退出。
                    api.prevent_close();
                    quit_app(window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_tasks,
            get_settings,
            take_launch_url,
            add_download,
            start_task,
            pause_task,
            cancel_task,
            delete_task,
            save_settings,
            resolve_media,
            reveal_task_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running RDM desktop");
}
