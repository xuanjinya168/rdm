//! 把下载管理器、解析器和本地桥接到同一条界面事件队列上。
//!
//! `DownloadManager` 的调度器依赖 tokio，因此运行时比 GPUI 窗口活得更久，
//! 并且只在后台线程里 `block_on`，不占用界面线程。

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use rdm_bridge::BridgeCommand;
use rdm_domain::config::{AppSettings, SettingsStore};
use rdm_http::ProviderRegistry;
use rdm_resolver::{ProxyConfig, ResolverRegistry};
use rdm_service::DownloadManager;
use rdm_storage::DownloadDatabase;

use crate::events::AppEvent;

pub struct Session {
    pub manager: DownloadManager,
    pub settings_store: SettingsStore,
    pub settings: AppSettings,
    pub resolvers: Arc<Mutex<Arc<ResolverRegistry>>>,
    pub event_rx: Receiver<AppEvent>,
    pub event_tx: Sender<AppEvent>,
    pub launch_url: Option<String>,
    pub force_quit: Arc<AtomicBool>,
    pub minimize_to_tray: Arc<AtomicBool>,
}

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("rdm-tokio")
            .build()
            .expect("tokio runtime")
    })
}

pub fn proxy_from(settings: &AppSettings) -> ProxyConfig {
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

pub fn bootstrap(launch_url: Option<String>) -> anyhow::Result<Session> {
    let database = Arc::new(DownloadDatabase::open(None)?);
    let settings_store = SettingsStore::new(None);
    let settings = settings_store.load();
    let (event_tx, event_rx) = mpsc::channel();
    let manager = runtime().block_on(async {
        DownloadManager::new(
            database,
            settings.clone(),
            Arc::new(ProviderRegistry::default()),
        )
        .map(|manager| manager.with_ffmpeg(bundled_ffmpeg()))
    })?;
    let progress_tx = event_tx.clone();
    manager.add_listener(Arc::new(move |task, speed| {
        let _ = progress_tx.send(AppEvent::Progress { task, speed });
    }));

    let (bridge_tx, bridge_rx) = mpsc::channel::<BridgeCommand>();
    let bridge_events = event_tx.clone();
    std::thread::spawn(move || {
        while let Ok(command) = bridge_rx.recv() {
            if bridge_events.send(AppEvent::Bridge(command)).is_err() {
                break;
            }
        }
    });
    runtime().spawn(rdm_bridge::serve(bridge_tx));

    let resolvers = Arc::new(ResolverRegistry::new(proxy_from(&settings))?);
    log::info!(
        "RDM started; {} task(s) restored from database",
        manager.all_tasks().len()
    );
    Ok(Session {
        minimize_to_tray: Arc::new(AtomicBool::new(settings.minimize_to_tray)),
        force_quit: Arc::new(AtomicBool::new(false)),
        manager,
        settings_store,
        settings,
        resolvers: Arc::new(Mutex::new(resolvers)),
        event_rx,
        event_tx,
        launch_url,
    })
}

fn bundled_ffmpeg() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let path = directory.join(name);
    path.is_file().then_some(path)
}

/// 关机时让调度器把进行中的任务写成可续传状态。
pub fn shutdown(manager: DownloadManager, tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        runtime().block_on(manager.shutdown());
        let _ = tx.send(AppEvent::ShutdownFinished);
    });
}
