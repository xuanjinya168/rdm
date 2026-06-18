//! Tauri backend for the RDM desktop app.
//!
//! Command layer over [`rdm_service::DownloadManager`] plus desktop
//! integration ported from the Python app: single-instance + URL handoff,
//! system tray with close-to-tray, graceful shutdown on quit, and opening a
//! task's folder. The frontend subscribes to `task://update` for live progress
//! and to `rdm://open-url` / `rdm://new-download` for tray/second-instance
//! triggers.

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

/// Shared application state managed by Tauri.
struct AppState {
    manager: DownloadManager,
    settings_store: SettingsStore,
    /// URL supplied to the first process on launch. The frontend takes it
    /// after its event listeners are registered, avoiding a startup race.
    launch_url: Mutex<Option<String>>,
    /// Resolves social-media / web post URLs into downloadable media items.
    /// Held behind a mutex so it can be rebuilt when the proxy setting changes
    /// without restarting the app.
    resolvers: Mutex<Arc<ResolverRegistry>>,
    /// Set when the user really wants to quit, so the close handler exits
    /// instead of hiding to the tray.
    force_quit: AtomicBool,
}

/// Translate the proxy portion of [`AppSettings`] into a resolver
/// [`ProxyConfig`]; an inactive config when proxying is disabled.
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

/// Rebuild the resolver registry from `settings`, replacing the one in state.
/// Logged on failure rather than propagated: a broken registry just means
/// media resolution is unavailable while downloads keep working.
fn rebuild_resolvers(state: &AppState, settings: &AppSettings) {
    match ResolverRegistry::new(proxy_from_settings(settings)) {
        Ok(registry) => {
            *state.resolvers.lock().unwrap() = Arc::new(registry);
        }
        Err(error) => log::error!("Failed to rebuild media resolvers: {error}"),
    }
}

/// Payload pushed to the frontend on each progress update.
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
) -> Result<DownloadTask, String> {
    let destination = destination
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| state.manager.settings().download_dir);
    state
        .manager
        .add_download(
            &url,
            std::path::Path::new(&destination),
            connections,
            filename.as_deref().unwrap_or_default(),
            sha256.as_deref().unwrap_or_default(),
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
fn save_settings(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    // The resolver client is built once and held behind a mutex; rebuild it
    // only when the proxy actually changes, since downloads read settings live.
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

/// Resolve a social-media / web post URL into its downloadable media items.
///
/// The registry is cloned out of state before the first `.await` so the command
/// future does not borrow `State` across suspension points.
#[tauri::command]
async fn resolve_media(state: State<'_, AppState>, url: String) -> Result<ResolvedPost, String> {
    let resolvers = state.resolvers.lock().unwrap().clone();
    resolvers
        .resolve(url.trim())
        .await
        .map_err(|error| error.to_string())
}

/// Open a task's destination folder in the system file manager.
#[tauri::command]
fn open_folder(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let _ = std::fs::create_dir_all(&path);
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|error| error.to_string())
}

/// The first http/https URL among CLI args, mirroring Python `first_http_url`.
fn first_http_url(args: &[String]) -> Option<String> {
    args.iter().find(|arg| is_http_url(arg)).cloned()
}

/// Reveal and focus the main window.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Begin a graceful quit: pause/persist in-flight downloads, then exit.
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

/// Build and run the Tauri application.
pub fn run() {
    let mut builder = tauri::Builder::default();

    // Single-instance must be registered first: a second launch hands its URL
    // to the running instance (which surfaces the window and opens the add
    // dialog) and then exits.
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
        // Rotating log file in the app log dir (ports PyDM's configure_logging):
        // when `rdm.log` exceeds the size cap it is rotated and only the most
        // recent backup is kept, so disk use stays bounded. Also mirrors to
        // stdout for `app:dev`.
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

            // DownloadManager spawns its scheduler with tokio::spawn, so it must
            // be constructed inside a runtime context; Tauri's async runtime is
            // tokio, and the spawned scheduler outlives this block.
            let manager = tauri::async_runtime::block_on(async {
                DownloadManager::new(database, settings.clone(), Arc::new(ProviderRegistry::default()))
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

            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                let minimize = state.manager.settings().minimize_to_tray;
                if minimize && !state.force_quit.load(Ordering::SeqCst) {
                    // Keep downloading in the background.
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    // Exit only after engines have wound down.
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
            open_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running RDM desktop");
}
