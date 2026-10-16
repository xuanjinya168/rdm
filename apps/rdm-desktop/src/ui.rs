//! GPUI 主窗口：下载列表、媒体解析、设置和确认对话框。

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use open_gpui::prelude::*;
use open_gpui::{
    actions, div, hsla, px, rgb, size, AnyElement, App, Bounds, Context, Entity, IntoElement,
    KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Render, SharedString,
    Subscription, Window, WindowBounds, WindowOptions,
};
use open_gpui_platform::application;
use rdm_bridge::{BridgeCommand, SniffedCandidate};
use rdm_domain::config::AppSettings;
use rdm_domain::validation::is_http_url;
use rdm_domain::{DownloadTask, TaskStatus};
use rdm_resolver::{ResolvedPost, ResolverRegistry};
use rdm_service::DownloadManager;

use crate::events::AppEvent;
use crate::format::{format_bytes, format_eta, format_speed, status_label};
use crate::forms::{validate_download, validate_settings, DownloadForm, SettingsForm};
use crate::session::{self, Session};
use crate::shell;
use crate::tasks::{
    can_delete, can_pause, can_start, matches_filter, merge_task_snapshots, TaskFilter,
};
use crate::widgets::{self, button, palette, Field, Palette};

actions!(
    rdm,
    [NewDownload, QuitApp, ShowDownloads, ShowMedia, Dismiss]
);

const CONNECTIONS: [u32; 8] = [1, 2, 4, 8, 12, 16, 24, 32];
const ACTIVE_LIMITS: [i64; 9] = [1, 2, 3, 4, 5, 8, 10, 15, 20];
const RETRIES: [i64; 9] = [0, 1, 2, 3, 4, 5, 8, 10, 20];
const SPEEDS_KB: [i64; 9] = [0, 512, 1024, 2048, 5120, 10240, 20480, 51200, 102400];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Downloads,
    Media,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Downloads,
    Network,
    Appearance,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    Info,
    Error,
    Success,
}

struct RowMenu {
    task_id: String,
    x: Pixels,
    y: Pixels,
}

struct SniffedUi {
    title: Option<String>,
    page_url: Option<String>,
    items: Vec<SniffedCandidate>,
    selected: HashSet<usize>,
    queued: HashSet<usize>,
    message: String,
}

pub struct AppView {
    manager: DownloadManager,
    settings_store: rdm_domain::config::SettingsStore,
    resolvers: Arc<Mutex<Arc<ResolverRegistry>>>,
    event_tx: std::sync::mpsc::Sender<AppEvent>,
    event_rx: std::sync::mpsc::Receiver<AppEvent>,
    force_quit: Arc<std::sync::atomic::AtomicBool>,
    minimize_to_tray: Arc<std::sync::atomic::AtomicBool>,
    tasks: Vec<DownloadTask>,
    speeds: std::collections::HashMap<String, f64>,
    settings: AppSettings,
    page: Page,
    filter: TaskFilter,
    focused: Option<String>,
    selected: HashSet<String>,
    banner: String,
    launch_url: Option<String>,
    quitting: bool,
    last_clip_poll: Instant,
    last_clipboard: String,
    _subs: Vec<Subscription>,

    quick_url: Entity<Field>,
    media_url: Entity<Field>,
    add_url: Entity<Field>,
    add_dest: Entity<Field>,
    add_name: Entity<Field>,
    add_sha: Entity<Field>,
    set_dir: Entity<Field>,
    set_proxy_url: Entity<Field>,
    set_proxy_user: Entity<Field>,
    set_proxy_pass: Entity<Field>,

    add_open: bool,
    add_connections: u32,
    add_referrer: String,
    add_error: String,
    open_menu: Option<&'static str>,
    settings_open: bool,
    settings_section: SettingsSection,
    set_active: i64,
    set_connections: i64,
    set_retry: i64,
    set_speed_kb: i64,
    set_clipboard: bool,
    set_tray: bool,
    set_hls: bool,
    set_theme: String,
    set_proxy: bool,
    settings_error: String,
    delete_targets: Vec<DownloadTask>,
    sniffed: Option<SniffedUi>,
    context_menu: Option<RowMenu>,
    media_loading: bool,
    media_inflight: Option<String>,
    media_message: String,
    media_kind: MessageKind,
    post: Option<ResolvedPost>,
    queued_media: HashSet<usize>,
}

impl AppView {
    fn new(session: Session, window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.set_window_title("RDM");
        let quick_url =
            cx.new(|cx| Field::new(cx, "quick-url", "粘贴文件地址，按 Enter 添加", false));
        let media_url = cx.new(|cx| {
            Field::new(
                cx,
                "media-url",
                "粘贴 Twitter / X、Instagram 或 Threads 帖子地址",
                false,
            )
        });
        let add_url = cx.new(|cx| Field::new(cx, "add-url", "https://example.com/file.zip", false));
        let add_dest = cx.new(|cx| Field::new(cx, "add-dest", "保存目录", false));
        let add_name = cx.new(|cx| Field::new(cx, "add-name", "留空则从服务器自动识别", false));
        let add_sha = cx.new(|cx| Field::new(cx, "add-sha", "可选，64 位十六进制", false));
        let set_dir = cx.new(|cx| Field::new(cx, "set-dir", "默认下载目录", false));
        let set_proxy_url =
            cx.new(|cx| Field::new(cx, "proxy-url", "http://127.0.0.1:7890", false));
        let set_proxy_user = cx.new(|cx| Field::new(cx, "proxy-user", "用户名", false));
        let set_proxy_pass = cx.new(|cx| Field::new(cx, "proxy-pass", "密码", true));

        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(
            &quick_url,
            window,
            |this, _, event: &widgets::FieldEvent, window, cx| {
                if matches!(event, widgets::FieldEvent::Submit)
                    && this.page == Page::Downloads
                    && !this.add_open
                {
                    this.quick_add(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &media_url,
            window,
            |this, _, event: &widgets::FieldEvent, _, cx| {
                if matches!(event, widgets::FieldEvent::Submit) && this.page == Page::Media {
                    this.resolve_media(cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &add_url,
            window,
            |this, _, event: &widgets::FieldEvent, _, cx| {
                if matches!(event, widgets::FieldEvent::Submit) && this.add_open {
                    this.submit_add(cx);
                }
            },
        ));

        let minimize = Arc::clone(&session.minimize_to_tray);
        let force = Arc::clone(&session.force_quit);
        let close_tx = session.event_tx.clone();
        window.on_window_should_close(cx, move |window, cx| {
            if minimize.load(Ordering::SeqCst) && !force.load(Ordering::SeqCst) {
                window.minimize_window();
                cx.hide();
                false
            } else {
                force.store(true, Ordering::SeqCst);
                let _ = close_tx.send(AppEvent::Quit);
                false
            }
        });

        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            if this
                .update_in(cx, |view, window, cx| view.poll(window, cx))
                .is_err()
            {
                break;
            }
        })
        .detach();

        let colors = palette(&session.settings.theme);
        for field in [
            &quick_url,
            &media_url,
            &add_url,
            &add_dest,
            &add_name,
            &add_sha,
            &set_dir,
            &set_proxy_url,
            &set_proxy_user,
            &set_proxy_pass,
        ] {
            field.update(cx, |field, cx| field.set_palette(colors, cx));
        }

        let mut view = Self {
            tasks: session.manager.all_tasks(),
            speeds: std::collections::HashMap::new(),
            settings: session.settings,
            manager: session.manager,
            settings_store: session.settings_store,
            resolvers: session.resolvers,
            event_tx: session.event_tx,
            event_rx: session.event_rx,
            force_quit: session.force_quit,
            minimize_to_tray: session.minimize_to_tray,
            page: Page::Downloads,
            filter: TaskFilter::All,
            focused: None,
            selected: HashSet::new(),
            banner: String::new(),
            launch_url: session.launch_url,
            quitting: false,
            last_clip_poll: Instant::now(),
            last_clipboard: String::new(),
            _subs: subs,
            quick_url,
            media_url,
            add_url,
            add_dest,
            add_name,
            add_sha,
            set_dir,
            set_proxy_url,
            set_proxy_user,
            set_proxy_pass,
            add_open: false,
            add_connections: 8,
            add_referrer: String::new(),
            add_error: String::new(),
            open_menu: None,
            settings_open: false,
            settings_section: SettingsSection::Downloads,
            set_active: 3,
            set_connections: 8,
            set_retry: 4,
            set_speed_kb: 0,
            set_clipboard: true,
            set_tray: true,
            set_hls: false,
            set_theme: "dark".to_string(),
            set_proxy: false,
            settings_error: String::new(),
            delete_targets: Vec::new(),
            sniffed: None,
            context_menu: None,
            media_loading: false,
            media_inflight: None,
            media_message: String::new(),
            media_kind: MessageKind::Info,
            post: None,
            queued_media: HashSet::new(),
        };
        view.recolor(cx);
        if let Some(url) = view.launch_url.take() {
            view.open_add(&url, "", "", window, cx);
        }
        view
    }

    fn recolor(&mut self, cx: &mut Context<Self>) {
        let colors = palette(&self.settings.theme);
        for field in self.fields() {
            field.update(cx, |field, cx| field.set_palette(colors, cx));
        }
    }

    fn fields(&self) -> [&Entity<Field>; 10] {
        [
            &self.quick_url,
            &self.media_url,
            &self.add_url,
            &self.add_dest,
            &self.add_name,
            &self.add_sha,
            &self.set_dir,
            &self.set_proxy_url,
            &self.set_proxy_user,
            &self.set_proxy_pass,
        ]
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut dirty = false;
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event, window, cx);
            dirty = true;
        }
        if self.last_clip_poll.elapsed() >= Duration::from_millis(1200) {
            self.last_clip_poll = Instant::now();
            if self.poll_clipboard(cx) {
                dirty = true;
            }
        }
        if dirty {
            cx.notify();
        }
    }

    fn handle_event(&mut self, event: AppEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            AppEvent::Progress { task, speed } => self.apply_progress(task, speed),
            AppEvent::Bridge(BridgeCommand::Activate) => cx.activate(true),
            AppEvent::Bridge(BridgeCommand::ExternalDownload(download)) => {
                self.open_add(
                    &download.url,
                    download.filename.as_deref().unwrap_or(""),
                    download.referrer.as_deref().unwrap_or(""),
                    window,
                    cx,
                );
                cx.activate(true);
            }
            AppEvent::Bridge(BridgeCommand::SniffedMedia(media)) => {
                let selected = media
                    .candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| downloadable(item))
                    .map(|(index, _)| index)
                    .collect();
                self.sniffed = Some(SniffedUi {
                    title: media.page_title,
                    page_url: media.page_url,
                    items: media.candidates,
                    selected,
                    queued: HashSet::new(),
                    message: String::new(),
                });
                self.page = Page::Downloads;
                cx.activate(true);
            }
            AppEvent::MediaResolved { url, result } => {
                if self.media_inflight.as_deref() != Some(url.as_str()) {
                    return;
                }
                self.media_loading = false;
                self.media_inflight = None;
                match result {
                    Ok(post) => {
                        let count = post.media.len();
                        self.post = Some(post);
                        self.queued_media.clear();
                        self.media_message = format!("解析成功，找到 {count} 个媒体文件。");
                        self.media_kind = MessageKind::Success;
                    }
                    Err(error) => {
                        self.post = None;
                        self.media_message = error;
                        self.media_kind = MessageKind::Error;
                    }
                }
            }
            AppEvent::NewDownload => {
                self.open_add("", "", "", window, cx);
                cx.activate(true);
            }
            AppEvent::Quit => self.begin_shutdown(),
            AppEvent::ShutdownFinished => cx.quit(),
        }
    }

    fn apply_progress(&mut self, task: DownloadTask, speed: f64) {
        let previous = self
            .tasks
            .iter()
            .find(|item| item.id == task.id)
            .map(|item| item.status);
        if previous.is_some() && previous != Some(task.status) {
            match task.status {
                TaskStatus::Completed => shell::notify("下载完成", &display_name(&task)),
                TaskStatus::Failed => shell::notify("下载失败", &display_name(&task)),
                _ => {}
            }
        }
        let id = task.id.clone();
        self.tasks = merge_task_snapshots(std::mem::take(&mut self.tasks), [task]);
        self.speeds.insert(id, speed);
    }

    fn poll_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.clipboard_monitoring {
            return false;
        }
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return false;
        };
        let Ok(text) = clipboard.get_text() else {
            return false;
        };
        let text = text.trim();
        if text.is_empty() || text == self.last_clipboard || !is_http_url(text) {
            return false;
        }
        self.last_clipboard = text.to_string();
        let value = text.to_string();
        self.quick_url
            .update(cx, |field, cx| field.set_text(value, cx));
        true
    }

    fn begin_shutdown(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.force_quit.store(true, Ordering::SeqCst);
        session::shutdown(self.manager.clone(), self.event_tx.clone());
    }

    fn open_add(
        &mut self,
        url: &str,
        filename: &str,
        referrer: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = Page::Downloads;
        self.add_open = true;
        self.add_error.clear();
        self.add_referrer = referrer.to_string();
        self.add_connections = self.settings.default_connections.clamp(1, 32) as u32;
        if !CONNECTIONS.contains(&self.add_connections) {
            self.add_connections = 8;
        }
        let destination = self.settings.download_dir.clone();
        self.add_url.update(cx, |field, cx| field.set_text(url, cx));
        self.add_dest
            .update(cx, |field, cx| field.set_text(destination, cx));
        self.add_name
            .update(cx, |field, cx| field.set_text(filename, cx));
        self.add_sha.update(cx, |field, cx| field.set_text("", cx));
        let handle = self.add_url.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    }

    fn quick_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.quick_url.read(cx).text().trim().to_string();
        if !is_http_url(&url) {
            self.banner = "请输入有效的 HTTP 或 HTTPS 地址。".to_string();
            cx.notify();
            return;
        }
        self.banner.clear();
        self.open_add(&url, "", "", window, cx);
        self.quick_url
            .update(cx, |field, cx| field.set_text("", cx));
        cx.notify();
    }

    fn submit_add(&mut self, cx: &mut Context<Self>) {
        let form = DownloadForm {
            url: self.add_url.read(cx).text().to_string(),
            destination: self.add_dest.read(cx).text().to_string(),
            filename: self.add_name.read(cx).text().to_string(),
            connections: self.add_connections,
            sha256: self.add_sha.read(cx).text().to_string(),
        };
        let value = match validate_download(&form) {
            Ok(value) => value,
            Err(error) => {
                self.add_error = error;
                cx.notify();
                return;
            }
        };
        match self.manager.add_download_with_referrer(
            &value.url,
            Path::new(&value.destination),
            Some(value.connections),
            &value.filename,
            &value.sha256,
            &self.add_referrer,
        ) {
            Ok(task) => {
                self.upsert(task);
                self.add_open = false;
                self.add_referrer.clear();
                self.banner.clear();
            }
            Err(error) => self.add_error = error.to_string(),
        }
        cx.notify();
    }

    fn upsert(&mut self, task: DownloadTask) {
        if let Some(slot) = self.tasks.iter_mut().find(|item| item.id == task.id) {
            *slot = task;
        } else {
            self.tasks.push(task);
        }
    }

    fn enqueue(&mut self, url: &str, filename: &str, referrer: &str) -> Result<(), String> {
        let destination = std::path::PathBuf::from(&self.settings.download_dir);
        let task = self
            .manager
            .add_download_with_referrer(url, &destination, None, filename, "", referrer)
            .map_err(|error| error.to_string())?;
        self.upsert(task);
        Ok(())
    }

    fn visible_tasks(&self) -> Vec<DownloadTask> {
        let mut tasks: Vec<_> = self
            .tasks
            .iter()
            .filter(|task| matches_filter(task, self.filter))
            .cloned()
            .collect();
        tasks.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        tasks
    }

    fn focused_task(&self) -> Option<DownloadTask> {
        let id = self.focused.as_ref()?;
        self.tasks.iter().find(|task| &task.id == id).cloned()
    }

    fn selected_tasks(&self) -> Vec<DownloadTask> {
        self.tasks
            .iter()
            .filter(|task| self.selected.contains(&task.id))
            .cloned()
            .collect()
    }

    fn targets(&self, can: fn(&DownloadTask) -> bool) -> Vec<String> {
        let pool = if self.selected.is_empty() {
            self.focused_task().into_iter().collect::<Vec<_>>()
        } else {
            self.selected_tasks()
        };
        pool.into_iter().filter(can).map(|task| task.id).collect()
    }

    fn run_targets(
        &mut self,
        can: fn(&DownloadTask) -> bool,
        op: impl Fn(&DownloadManager, &str) -> Result<(), String>,
    ) {
        let ids = self.targets(can);
        if ids.is_empty() {
            return;
        }
        let mut failed = 0;
        for id in ids {
            if op(&self.manager, &id).is_err() {
                failed += 1;
            }
        }
        self.banner = if failed == 0 {
            String::new()
        } else {
            format!("{failed} 个任务操作失败。")
        };
    }

    fn request_delete(&mut self, tasks: Vec<DownloadTask>) {
        let deletable: Vec<_> = tasks.into_iter().filter(can_delete).collect();
        if deletable.is_empty() {
            self.banner = "请先暂停或取消任务再删除。".to_string();
            return;
        }
        self.banner.clear();
        self.context_menu = None;
        self.delete_targets = deletable;
    }

    fn confirm_delete(&mut self, with_file: bool) {
        let targets = std::mem::take(&mut self.delete_targets);
        let mut removed = HashSet::new();
        let mut failed = 0;
        for task in targets {
            match self.manager.delete(&task.id, with_file) {
                Ok(true) => {
                    removed.insert(task.id);
                }
                _ => failed += 1,
            }
        }
        self.tasks.retain(|task| !removed.contains(&task.id));
        self.selected.retain(|id| !removed.contains(id));
        if self.focused.as_ref().is_some_and(|id| removed.contains(id)) {
            self.focused = None;
        }
        if failed > 0 {
            self.banner = format!("{failed} 个任务删除失败或仍在进行中。");
        }
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        self.set_active = settings.max_active_downloads;
        self.set_connections = settings.default_connections;
        self.set_retry = settings.retry_count;
        self.set_speed_kb = settings.speed_limit_bytes / 1024;
        self.set_clipboard = settings.clipboard_monitoring;
        self.set_tray = settings.minimize_to_tray;
        self.set_hls = settings.hls_transcode;
        self.set_theme = settings.theme.clone();
        self.set_proxy = settings.proxy_enabled;
        self.settings_error.clear();
        self.settings_section = SettingsSection::Downloads;
        self.settings_open = true;
        self.set_dir
            .update(cx, |field, cx| field.set_text(&settings.download_dir, cx));
        self.set_proxy_url
            .update(cx, |field, cx| field.set_text(&settings.proxy_url, cx));
        self.set_proxy_user
            .update(cx, |field, cx| field.set_text(&settings.proxy_username, cx));
        self.set_proxy_pass
            .update(cx, |field, cx| field.set_text(&settings.proxy_password, cx));
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let form = SettingsForm {
            download_dir: self.set_dir.read(cx).text().to_string(),
            max_active: self.set_active,
            connections: self.set_connections,
            retry: self.set_retry,
            speed_kb: self.set_speed_kb,
            clipboard: self.set_clipboard,
            tray: self.set_tray,
            hls_transcode: self.set_hls,
            theme: self.set_theme.clone(),
            proxy_enabled: self.set_proxy,
            proxy_url: self.set_proxy_url.read(cx).text().to_string(),
            proxy_username: self.set_proxy_user.read(cx).text().to_string(),
            proxy_password: self.set_proxy_pass.read(cx).text().to_string(),
        };
        let draft = match validate_settings(&form) {
            Ok(draft) => draft,
            Err(error) => {
                self.settings_error = error;
                cx.notify();
                return;
            }
        };
        let old_proxy = session::proxy_from(&self.settings);
        match self.settings_store.save(&draft) {
            Ok(saved) => {
                self.manager.update_settings(saved.clone());
                if session::proxy_from(&saved) != old_proxy {
                    match ResolverRegistry::new(session::proxy_from(&saved)) {
                        Ok(registry) => *self.resolvers.lock().unwrap() = Arc::new(registry),
                        Err(error) => log::error!("Failed to rebuild media resolvers: {error}"),
                    }
                }
                self.minimize_to_tray
                    .store(saved.minimize_to_tray, Ordering::SeqCst);
                self.settings = saved;
                self.settings_open = false;
                self.recolor(cx);
            }
            Err(error) => self.settings_error = error.to_string(),
        }
        cx.notify();
    }

    fn resolve_media(&mut self, cx: &mut Context<Self>) {
        let url = self.media_url.read(cx).text().trim().to_string();
        if !is_http_url(&url) {
            self.media_message = "请输入有效的网页或媒体地址。".to_string();
            self.media_kind = MessageKind::Error;
            cx.notify();
            return;
        }
        self.media_loading = true;
        self.media_inflight = Some(url.clone());
        self.media_message = "正在解析链接…".to_string();
        self.media_kind = MessageKind::Info;
        self.post = None;
        self.queued_media.clear();
        let resolvers = Arc::clone(&self.resolvers);
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let result = session::runtime().block_on(async {
                let registry = resolvers.lock().unwrap().clone();
                registry
                    .resolve(&url)
                    .await
                    .map_err(|error| error.to_string())
            });
            let _ = tx.send(AppEvent::MediaResolved { url, result });
        });
        cx.notify();
    }

    fn input_focused(&self, window: &Window, cx: &App) -> bool {
        let Some(focused) = window.focused(cx) else {
            return false;
        };
        self.fields()
            .into_iter()
            .any(|field| field.read(cx).focus_handle == focused)
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.delete_targets.is_empty() {
            self.delete_targets.clear();
        } else if self.add_open {
            self.add_open = false;
        } else if self.settings_open {
            self.settings_open = false;
        } else if self.sniffed.is_some() {
            self.sniffed = None;
        } else {
            self.context_menu = None;
            self.open_menu = None;
        }
        cx.notify();
    }

    fn toggle_menu(&mut self, id: &'static str) {
        self.open_menu = if self.open_menu == Some(id) {
            None
        } else {
            Some(id)
        };
    }

    fn choose(&mut self, menu: &str, index: usize) {
        match menu {
            "add-conn" => self.add_connections = CONNECTIONS[index],
            "set-active" => self.set_active = ACTIVE_LIMITS[index],
            "set-conn" => self.set_connections = CONNECTIONS[index] as i64,
            "set-retry" => self.set_retry = RETRIES[index],
            "set-speed" => self.set_speed_kb = SPEEDS_KB[index],
            _ => {}
        }
        self.open_menu = None;
    }

    fn render_dropdown(
        &mut self,
        id: &'static str,
        current: String,
        choices: &[String],
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open_menu == Some(id);
        let mut column = div().flex().flex_col().gap(px(4.)).child(
            button(id, current, colors.raised, colors.text, true).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.toggle_menu(id);
                    cx.stop_propagation();
                    cx.notify();
                }),
            ),
        );
        if open {
            let mut list = div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .p(px(4.))
                .rounded(px(7.))
                .bg(rgb(colors.panel))
                .border_1()
                .border_color(rgb(colors.line));
            for (index, choice) in choices.iter().cloned().enumerate() {
                let menu = id;
                list = list.child(
                    button(
                        SharedString::from(format!("{id}-{index}")),
                        choice,
                        colors.deep,
                        colors.text,
                        true,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.choose(menu, index);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
                );
            }
            column = column.child(list);
        }
        column.into_any_element()
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(&self.settings.theme);
        let version = env!("CARGO_PKG_VERSION");
        div()
            .id("rdm-root")
            .relative()
            .size_full()
            .flex()
            .bg(rgb(colors.bg))
            .text_color(rgb(colors.text))
            .text_size(px(13.))
            .on_action(cx.listener(|this, _: &NewDownload, window, cx| {
                this.open_add("", "", "", window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuitApp, _, cx| {
                this.begin_shutdown();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowDownloads, _, cx| {
                this.page = Page::Downloads;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowMedia, _, cx| {
                this.page = Page::Media;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.dismiss(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.context_menu.is_some() || this.open_menu.is_some() {
                        this.context_menu = None;
                        this.open_menu = None;
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "delete" && !this.input_focused(window, cx) {
                    let targets = if this.selected.is_empty() {
                        this.focused_task().into_iter().collect()
                    } else {
                        this.selected_tasks()
                    };
                    this.request_delete(targets);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(self.sidebar(colors, version, cx))
            .child(self.workspace(colors, cx))
            .children(self.overlay(colors, window, cx))
    }
}

impl AppView {
    fn sidebar(&mut self, colors: Palette, version: &str, cx: &mut Context<Self>) -> AnyElement {
        let downloads_bg = if self.page == Page::Downloads {
            colors.accent
        } else {
            colors.sidebar
        };
        let downloads_fg = if self.page == Page::Downloads {
            0xffffff
        } else {
            colors.text
        };
        let media_bg = if self.page == Page::Media {
            colors.accent
        } else {
            colors.sidebar
        };
        let media_fg = if self.page == Page::Media {
            0xffffff
        } else {
            colors.text
        };
        div()
            .w(px(220.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .py(px(16.))
            .bg(rgb(colors.sidebar))
            .border_r_1()
            .border_color(rgb(colors.line))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child("工作台"),
            )
            .child(
                button(
                    "nav-downloads",
                    "下载中心",
                    downloads_bg,
                    downloads_fg,
                    true,
                )
                .w_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.page = Page::Downloads;
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("nav-media", "媒体解析", media_bg, media_fg, true)
                    .w_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.page = Page::Media;
                            cx.notify();
                        }),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(8.))
                            .h(px(8.))
                            .rounded(px(4.))
                            .bg(rgb(colors.success)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(12.)).child("下载引擎"))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors.muted))
                                    .child("本地服务运行中"),
                            ),
                    ),
            )
            .child(
                button("open-settings", "设置", colors.raised, colors.text, true)
                    .w_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.open_settings(cx);
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors.muted))
                    .child(format!("RDM {version}")),
            )
            .into_any_element()
    }

    fn workspace(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = if self.page == Page::Downloads {
            self.downloads_page(colors, cx)
        } else {
            self.media_page(colors, cx)
        };
        div()
            .flex_1()
            .h_full()
            .min_w(px(0.))
            .overflow_hidden()
            .p(px(16.))
            .child(page)
            .into_any_element()
    }

    fn downloads_page(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let visible = self.visible_tasks();
        let counts = count_tasks(&self.tasks);
        let filter = self.filter;
        let mut rows = Vec::new();
        if visible.is_empty() {
            rows.push(self.empty_row(colors, cx));
        } else {
            for task in &visible {
                rows.push(self.task_row(task, colors, cx));
            }
        }
        let banner = self.banner.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().flex_1().child(self.quick_url.clone()))
                    .child(
                        button("quick-add", "添加", colors.accent, 0xffffff, true).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.quick_add(window, cx)),
                        ),
                    ),
            )
            .when(!banner.is_empty(), |el| {
                el.child(div().text_color(rgb(colors.danger)).child(banner))
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .rounded(px(10.))
                    .bg(rgb(colors.panel))
                    .border_1()
                    .border_color(rgb(colors.line))
                    .child(self.task_toolbar(colors, visible.len(), cx))
                    .child(self.filter_bar(colors, counts, filter, cx))
                    .child(self.table_head(colors, &visible, cx))
                    .child(
                        div()
                            .id("task-scroll")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
            .into_any_element()
    }

    fn task_toolbar(
        &mut self,
        colors: Palette,
        visible: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected.len();
        let start_enabled = !self.targets(can_start).is_empty();
        let pause_enabled = !self.targets(can_pause).is_empty();
        let delete_enabled = !self.targets(can_delete).is_empty();
        let label = if selected > 0 {
            format!("已选 {selected} 项")
        } else {
            format!("{visible} 个项目")
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(rgb(colors.line))
            .child(
                div().flex().flex_col().child("下载任务").child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(label),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        button(
                            "start-tasks",
                            "开始",
                            colors.raised,
                            colors.text,
                            start_enabled,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if start_enabled {
                                    this.run_targets(can_start, |manager, id| {
                                        manager.start(id).map_err(|error| error.to_string())
                                    });
                                }
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(
                            "pause-tasks",
                            "暂停",
                            colors.raised,
                            colors.text,
                            pause_enabled,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if pause_enabled {
                                    this.run_targets(can_pause, |manager, id| {
                                        manager.pause(id).map_err(|error| error.to_string())
                                    });
                                }
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(
                            "reveal-task",
                            "定位",
                            colors.raised,
                            colors.text,
                            self.focused.is_some() && selected == 0,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if let Some(task) = this.focused_task() {
                                    shell::reveal_task(&task);
                                }
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(
                            "delete-tasks",
                            "删除",
                            colors.danger,
                            0xffffff,
                            delete_enabled,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let targets = if this.selected.is_empty() {
                                    this.focused_task().into_iter().collect()
                                } else {
                                    this.selected_tasks()
                                };
                                this.request_delete(targets);
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn filter_bar(
        &mut self,
        colors: Palette,
        counts: [usize; 4],
        filter: TaskFilter,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = [
            (TaskFilter::All, "全部", counts[0], "filter-all"),
            (TaskFilter::Active, "进行中", counts[1], "filter-active"),
            (TaskFilter::Completed, "已完成", counts[2], "filter-done"),
            (TaskFilter::Other, "其他", counts[3], "filter-other"),
        ];
        let mut row = div().flex().gap(px(6.)).px(px(12.)).py(px(8.));
        for (kind, label, count, id) in items {
            let active = filter == kind;
            let bg = if active { colors.accent } else { colors.deep };
            let fg = if active { 0xffffff } else { colors.text };
            row = row.child(
                button(id, format!("{label} {count}"), bg, fg, true).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.filter = kind;
                        cx.notify();
                    }),
                ),
            );
        }
        row.into_any_element()
    }

    fn table_head(
        &mut self,
        colors: Palette,
        visible: &[DownloadTask],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids: Vec<String> = visible.iter().map(|task| task.id.clone()).collect();
        let all_selected = !ids.is_empty() && ids.iter().all(|id| self.selected.contains(id));
        div()
            .flex()
            .items_center()
            .px(px(8.))
            .h(px(32.))
            .text_size(px(12.))
            .text_color(rgb(colors.muted))
            .border_b_1()
            .border_color(rgb(colors.line))
            .child(check_box("check-all", all_selected, colors).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let turn_on = !ids.iter().all(|id| this.selected.contains(id));
                    for id in &ids {
                        if turn_on {
                            this.selected.insert(id.clone());
                        } else {
                            this.selected.remove(id);
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(div().flex_1().child("文件"))
            .child(div().w(px(78.)).child("大小"))
            .child(div().w(px(72.)).child("状态"))
            .child(div().w(px(120.)).child("进度"))
            .child(div().w(px(88.)).child("速度"))
            .child(div().w(px(88.)).child("剩余"))
            .into_any_element()
    }

    fn task_row(
        &mut self,
        task: &DownloadTask,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = task.id.clone();
        let menu_id = id.clone();
        let select_id = id.clone();
        let selected = self.selected.contains(&task.id);
        let focused = self.focused.as_deref() == Some(task.id.as_str());
        let speed = self.speeds.get(&task.id).copied().unwrap_or(0.0);
        let name = display_name(task);
        let size = task
            .total_size
            .map(format_bytes)
            .unwrap_or_else(|| "—".to_string());
        let speed_text = if task.status.is_active() {
            format_speed(speed)
        } else {
            "—".to_string()
        };
        let eta = if task.status.is_active() {
            format_eta(task.total_size, task.downloaded, speed)
        } else {
            "—".to_string()
        };
        let progress = (112.0 * task.progress() as f32).clamp(0.0, 112.0);
        let bg = if focused {
            colors.selected
        } else {
            colors.panel
        };
        let error = task.error.clone().unwrap_or_default();
        div()
            .id(SharedString::from(format!("row-{id}")))
            .flex()
            .items_center()
            .px(px(8.))
            .py(px(8.))
            .bg(rgb(bg))
            .border_b_1()
            .border_color(rgb(colors.line))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.focused = Some(id.clone());
                    if event.click_count >= 2 {
                        if let Some(task) = this.tasks.iter().find(|task| task.id == id).cloned() {
                            shell::reveal_task(&task);
                        }
                    }
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.focused = Some(menu_id.clone());
                    this.context_menu = Some(RowMenu {
                        task_id: menu_id.clone(),
                        x: event.position.x,
                        y: event.position.y,
                    });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                check_box(
                    SharedString::from(format!("sel-{select_id}")),
                    selected,
                    colors,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if !this.selected.insert(select_id.clone()) {
                            this.selected.remove(&select_id);
                        }
                        this.focused = Some(select_id.clone());
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(name)
                    .when(!error.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors.danger))
                                .child(error),
                        )
                    }),
            )
            .child(div().w(px(78.)).child(size))
            .child(
                div()
                    .w(px(72.))
                    .text_color(rgb(status_color(task.status, colors)))
                    .child(status_label(task.status)),
            )
            .child(
                div().w(px(120.)).child(
                    div()
                        .w(px(112.))
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(rgb(colors.track))
                        .child(
                            div()
                                .h(px(6.))
                                .w(px(progress))
                                .rounded(px(3.))
                                .bg(rgb(colors.accent)),
                        ),
                ),
            )
            .child(div().w(px(88.)).child(speed_text))
            .child(div().w(px(88.)).child(eta))
            .into_any_element()
    }

    fn empty_row(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .py(px(48.))
            .text_color(rgb(colors.muted))
            .child("还没有下载任务")
            .child("添加直接链接，或从媒体解析结果中创建任务")
            .child(
                button("empty-new", "新建下载", colors.accent, 0xffffff, true).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.open_add("", "", "", window, cx)),
                ),
            )
            .into_any_element()
    }

    fn media_page(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.media_loading;
        let message = self.media_message.clone();
        let kind = self.media_kind;
        let message_color = match kind {
            MessageKind::Error => colors.danger,
            MessageKind::Success => colors.success,
            MessageKind::Info => colors.info,
        };
        let post = self.post.clone();
        let queued = self.queued_media.clone();
        let download_dir = self.settings.download_dir.clone();
        let mut body = div().size_full().flex().flex_col().gap(px(12.)).child(
            div()
                .flex()
                .gap(px(8.))
                .child(div().flex_1().child(self.media_url.clone()))
                .child(
                    button(
                        "resolve-media",
                        if loading { "解析中…" } else { "解析" },
                        colors.accent,
                        0xffffff,
                        !loading,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if !this.media_loading {
                                this.resolve_media(cx);
                            }
                        }),
                    ),
                ),
        );
        if !message.is_empty() {
            body = body.child(div().text_color(rgb(message_color)).child(message));
        }
        if let Some(post) = post {
            body = body.child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child("解析结果")
                            .when(!post.title.is_empty(), |el| el.child(post.title.clone()))
                            .when(!post.text.is_empty(), |el| {
                                el.child(
                                    div().text_color(rgb(colors.muted)).child(post.text.clone()),
                                )
                            }),
                    )
                    .child(
                        button(
                            "download-all-media",
                            "全部下载",
                            colors.accent,
                            0xffffff,
                            !post.media.is_empty(),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let Some(post) = this.post.clone() else {
                                    return;
                                };
                                let referrer = post.source_url.clone();
                                for (index, item) in post.media.iter().enumerate() {
                                    if this.queued_media.contains(&index) {
                                        continue;
                                    }
                                    if this.enqueue(&item.url, &item.filename, &referrer).is_ok() {
                                        this.queued_media.insert(index);
                                    }
                                }
                                this.media_message = "已将全部媒体加入下载中心。".to_string();
                                this.media_kind = MessageKind::Success;
                                cx.notify();
                            }),
                        ),
                    ),
            );
            for (index, item) in post.media.iter().enumerate() {
                let done = queued.contains(&index);
                let label = if done { "已加入" } else { "下载" };
                let filename = item.filename.clone();
                let meta = format!(
                    "{}{}",
                    item.ext.to_uppercase(),
                    if item.width > 0 && item.height > 0 {
                        format!(" · {}×{}", item.width, item.height)
                    } else {
                        String::new()
                    }
                );
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(rgb(colors.panel))
                        .border_1()
                        .border_color(rgb(colors.line))
                        .child(
                            div().flex().flex_col().min_w(px(0.)).child(filename).child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors.muted))
                                    .child(meta),
                            ),
                        )
                        .child(
                            button(
                                SharedString::from(format!("media-{index}")),
                                label,
                                if done { colors.raised } else { colors.accent },
                                if done { colors.text } else { 0xffffff },
                                !done,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if this.queued_media.contains(&index) {
                                        return;
                                    }
                                    let Some(post) = this.post.clone() else {
                                        return;
                                    };
                                    let Some(item) = post.media.get(index) else {
                                        return;
                                    };
                                    let referrer = post.source_url.clone();
                                    match this.enqueue(&item.url, &item.filename, &referrer) {
                                        Ok(()) => {
                                            this.queued_media.insert(index);
                                        }
                                        Err(error) => {
                                            this.media_message = format!(
                                                "「{}」加入下载失败：{error}",
                                                item.filename
                                            );
                                            this.media_kind = MessageKind::Error;
                                        }
                                    }
                                    cx.notify();
                                }),
                            ),
                        ),
                );
            }
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(format!("下载将保存到默认目录：{download_dir}")),
            );
        } else if !loading {
            body = body.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(colors.muted))
                    .child("解析结果会显示在这里")
                    .child("粘贴帖子地址并点击「解析」"),
            );
        }
        body.into_any_element()
    }

    fn overlay(
        &mut self,
        colors: Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut layers = Vec::new();
        if let Some(menu) = self.context_menu.as_ref() {
            if let Some(task) = self
                .tasks
                .iter()
                .find(|task| task.id == menu.task_id)
                .cloned()
            {
                layers.push(self.context_menu_layer(&task, menu.x, menu.y, colors, cx));
            }
        }
        if self.add_open {
            layers.push(self.add_dialog(colors, cx));
        }
        if self.sniffed.is_some() {
            layers.push(self.sniffed_dialog(colors, cx));
        }
        if self.settings_open {
            layers.push(self.settings_dialog(colors, cx));
        }
        if !self.delete_targets.is_empty() {
            layers.push(self.delete_dialog(colors, cx));
        }
        layers
    }

    fn context_menu_layer(
        &mut self,
        task: &DownloadTask,
        x: Pixels,
        y: Pixels,
        colors: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = task.id.clone();
        let mut menu = div()
            .absolute()
            .left(x)
            .top(y)
            .w(px(180.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .bg(rgb(colors.panel))
            .border_1()
            .border_color(rgb(colors.line))
            .occlude();
        if can_start(task) {
            let action_id = id.clone();
            menu = menu.child(menu_item("menu-start", "开始/继续", colors).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let _ = this.manager.start(&action_id);
                    this.context_menu = None;
                    cx.notify();
                }),
            ));
        }
        if can_pause(task) {
            let action_id = id.clone();
            menu = menu.child(menu_item("menu-pause", "暂停", colors).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let _ = this.manager.pause(&action_id);
                    this.context_menu = None;
                    cx.notify();
                }),
            ));
        }
        if task.status != TaskStatus::Completed {
            let action_id = id.clone();
            menu = menu.child(menu_item("menu-cancel", "取消", colors).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let _ = this.manager.cancel(&action_id);
                    this.context_menu = None;
                    cx.notify();
                }),
            ));
        }
        let reveal_id = id.clone();
        menu = menu.child(
            menu_item("menu-reveal", "在文件夹中显示", colors).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(task) = this.tasks.iter().find(|task| task.id == reveal_id).cloned()
                    {
                        shell::reveal_task(&task);
                    }
                    this.context_menu = None;
                    cx.notify();
                }),
            ),
        );
        if can_delete(task) {
            let delete_id = id;
            menu = menu.child(menu_item("menu-delete", "删除任务", colors).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(task) = this.tasks.iter().find(|task| task.id == delete_id).cloned()
                    {
                        this.request_delete(vec![task]);
                    }
                    cx.notify();
                }),
            ));
        }
        menu.into_any_element()
    }

    fn add_dialog(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let error = self.add_error.clone();
        let choices: Vec<String> = CONNECTIONS.iter().copied().map(connection_label).collect();
        let current = connection_label(self.add_connections);
        let dropdown = self.render_dropdown("add-conn", current, &choices, colors, cx);
        self.modal(colors, cx, "add-overlay", |this, colors, cx| {
            div()
                .w(px(520.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(16.))
                .rounded(px(12.))
                .bg(rgb(colors.panel))
                .border_1()
                .border_color(rgb(colors.line))
                .child(div().text_size(px(18.)).child("新建下载"))
                .child(
                    div()
                        .text_color(rgb(colors.muted))
                        .child("添加下载地址并选择文件保存位置。"),
                )
                .child("下载地址")
                .child(this.add_url.clone())
                .child("保存位置")
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(div().flex_1().child(this.add_dest.clone()))
                        .child(
                            button("browse-add", "浏览…", colors.raised, colors.text, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let current = this.add_dest.read(cx).text().to_string();
                                        if let Some(path) = shell::pick_folder(&current) {
                                            this.add_dest
                                                .update(cx, |field, cx| field.set_text(path, cx));
                                        }
                                    }),
                                ),
                        ),
                )
                .child("文件名称")
                .child(this.add_name.clone())
                .child("并发连接")
                .child(dropdown)
                .child("SHA-256")
                .child(this.add_sha.clone())
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(
                        "服务器不支持分段时会自动降级为单连接；填写 SHA-256 后将在完成前校验文件。",
                    ),
                )
                .when(!error.is_empty(), |el| {
                    el.child(div().text_color(rgb(colors.danger)).child(error))
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            button("add-cancel", "取消", colors.raised, colors.text, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.add_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            button("add-submit", "开始下载", colors.accent, 0xffffff, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.submit_add(cx)),
                                ),
                        ),
                )
                .into_any_element()
        })
    }

    fn settings_dialog(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let section = self.settings_section;
        let error = self.settings_error.clone();
        let active_choices: Vec<String> = ACTIVE_LIMITS.iter().copied().map(active_label).collect();
        let connection_choices: Vec<String> =
            CONNECTIONS.iter().copied().map(connection_label).collect();
        let retry_choices: Vec<String> = RETRIES.iter().copied().map(retry_label).collect();
        let speed_choices: Vec<String> = SPEEDS_KB.iter().copied().map(speed_label).collect();
        let active_current = active_label(self.set_active);
        let connection_current = connection_label(self.set_connections as u32);
        let retry_current = retry_label(self.set_retry);
        let speed_current = speed_label(self.set_speed_kb);
        let active =
            self.render_dropdown("set-active", active_current, &active_choices, colors, cx);
        let connections = self.render_dropdown(
            "set-conn",
            connection_current,
            &connection_choices,
            colors,
            cx,
        );
        let retry = self.render_dropdown("set-retry", retry_current, &retry_choices, colors, cx);
        let speed = self.render_dropdown("set-speed", speed_current, &speed_choices, colors, cx);
        let clipboard = self.set_clipboard;
        let tray = self.set_tray;
        let hls = self.set_hls;
        let proxy = self.set_proxy;
        let theme = self.set_theme.clone();
        self.modal(colors, cx, "settings-overlay", move |this, colors, cx| {
            let mut content = div().flex().flex_col().gap(px(10.));
            if section == SettingsSection::Downloads {
                content = content
                    .child("默认目录")
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_1().child(this.set_dir.clone()))
                            .child(
                                button(
                                    "browse-settings",
                                    "浏览…",
                                    colors.raised,
                                    colors.text,
                                    true,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let current = this.set_dir.read(cx).text().to_string();
                                        if let Some(path) = shell::pick_folder(&current) {
                                            this.set_dir
                                                .update(cx, |field, cx| field.set_text(path, cx));
                                        }
                                    }),
                                ),
                            ),
                    )
                    .child("同时下载")
                    .child(active)
                    .child("默认连接")
                    .child(connections)
                    .child("失败重试")
                    .child(retry)
                    .child("全局限速")
                    .child(speed)
                    .child(flag(
                        this,
                        "clip",
                        "剪贴板监控",
                        "自动识别复制的 HTTP/HTTPS 地址",
                        clipboard,
                        colors,
                        cx,
                    ))
                    .child(flag(
                        this,
                        "tray",
                        "最小化到托盘",
                        "关闭主窗口后继续运行下载任务",
                        tray,
                        colors,
                        cx,
                    ))
                    .child(flag(
                        this,
                        "hls",
                        "流媒体下载后转码（GPU）",
                        "默认仅无损封装为 mp4",
                        hls,
                        colors,
                        cx,
                    ));
            } else if section == SettingsSection::Network {
                content = content
                    .child(flag(
                        this,
                        "proxy",
                        "启用代理",
                        "下载和媒体解析走同一个代理",
                        proxy,
                        colors,
                        cx,
                    ))
                    .child("代理地址")
                    .child(this.set_proxy_url.clone())
                    .child("用户名")
                    .child(this.set_proxy_user.clone())
                    .child("密码")
                    .child(this.set_proxy_pass.clone());
            } else {
                content = content.child("主题").child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            button(
                                "theme-dark",
                                "暗色",
                                if theme == "dark" {
                                    colors.accent
                                } else {
                                    colors.raised
                                },
                                if theme == "dark" {
                                    0xffffff
                                } else {
                                    colors.text
                                },
                                true,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_theme = "dark".to_string();
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            button(
                                "theme-light",
                                "亮色",
                                if theme == "light" {
                                    colors.accent
                                } else {
                                    colors.raised
                                },
                                if theme == "light" {
                                    0xffffff
                                } else {
                                    colors.text
                                },
                                true,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.set_theme = "light".to_string();
                                    cx.notify();
                                }),
                            ),
                        ),
                );
            }
            div()
                .w(px(640.))
                .max_h(px(640.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(16.))
                .rounded(px(12.))
                .bg(rgb(colors.panel))
                .border_1()
                .border_color(rgb(colors.line))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(div().text_size(px(18.)).child("设置"))
                        .child(
                            button("settings-close", "关闭", colors.raised, colors.text, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings_open = false;
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(section_button(
                            this,
                            "sec-down",
                            "下载设置",
                            SettingsSection::Downloads,
                            section,
                            colors,
                            cx,
                        ))
                        .child(section_button(
                            this,
                            "sec-net",
                            "网络代理",
                            SettingsSection::Network,
                            section,
                            colors,
                            cx,
                        ))
                        .child(section_button(
                            this,
                            "sec-look",
                            "外观",
                            SettingsSection::Appearance,
                            section,
                            colors,
                            cx,
                        )),
                )
                .child(
                    div()
                        .id("settings-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .child(content),
                )
                .when(!error.is_empty(), |el| {
                    el.child(div().text_color(rgb(colors.danger)).child(error))
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            button("settings-cancel", "取消", colors.raised, colors.text, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            button("settings-save", "保存设置", colors.accent, 0xffffff, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.save_settings(cx)),
                                ),
                        ),
                )
                .into_any_element()
        })
    }

    fn sniffed_dialog(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.sniffed.as_ref() else {
            return div().into_any_element();
        };
        let title = state.title.clone();
        let message = state.message.clone();
        let dir = self.settings.download_dir.clone();
        let rows: Vec<_> = state
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let selected = state.selected.contains(&index);
                let queued = state.queued.contains(&index);
                let name = item.filename.clone().unwrap_or_else(|| item.url.clone());
                let meta = candidate_meta(item);
                let allowed = downloadable(item);
                (index, name, meta, selected, queued, allowed)
            })
            .collect();
        self.modal(colors, cx, "sniff-overlay", move |_this, colors, cx| {
            let mut list = div().flex().flex_col().gap(px(6.));
            for (index, name, meta, selected, queued, allowed) in rows {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            check_box(
                                SharedString::from(format!("sniff-{index}")),
                                selected || queued,
                                colors,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let Some(state) = this.sniffed.as_mut() else {
                                        return;
                                    };
                                    if !allowed || state.queued.contains(&index) {
                                        return;
                                    }
                                    if !state.selected.insert(index) {
                                        state.selected.remove(&index);
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(name)
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(rgb(colors.muted))
                                        .child(meta),
                                ),
                        )
                        .child(if allowed {
                            if queued {
                                "已加入".to_string()
                            } else {
                                "可下载".to_string()
                            }
                        } else {
                            "仅识别".to_string()
                        }),
                );
            }
            div()
                .w(px(640.))
                .max_h(px(640.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(16.))
                .rounded(px(12.))
                .bg(rgb(colors.panel))
                .border_1()
                .border_color(rgb(colors.line))
                .child(
                    div()
                        .text_size(px(18.))
                        .child(title.unwrap_or_else(|| "检测到媒体".to_string())),
                )
                .child(
                    div()
                        .id("sniff-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .child(list),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .child(format!("保存到：{dir}")),
                )
                .when(!message.is_empty(), |el| el.child(message))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            button("sniff-close", "关闭", colors.raised, colors.text, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.sniffed = None;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            button("sniff-download", "下载所选", colors.accent, 0xffffff, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.download_sniffed();
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
                .into_any_element()
        })
    }

    fn download_sniffed(&mut self) {
        let Some(state) = self.sniffed.as_ref() else {
            return;
        };
        let targets: Vec<usize> = state
            .selected
            .iter()
            .copied()
            .filter(|index| !state.queued.contains(index))
            .collect();
        let mut ok = 0;
        let mut failed = 0;
        for index in targets {
            let Some(item) = self
                .sniffed
                .as_ref()
                .and_then(|state| state.items.get(index))
                .cloned()
            else {
                continue;
            };
            if !downloadable(&item) {
                continue;
            }
            let referrer = item
                .page_url
                .clone()
                .or_else(|| {
                    self.sniffed
                        .as_ref()
                        .and_then(|state| state.page_url.clone())
                })
                .unwrap_or_default();
            let filename = item.filename.clone().unwrap_or_default();
            match self.enqueue(&item.url, &filename, &referrer) {
                Ok(()) => {
                    if let Some(state) = self.sniffed.as_mut() {
                        state.queued.insert(index);
                        state.selected.remove(&index);
                    }
                    ok += 1;
                }
                Err(_) => failed += 1,
            }
        }
        if let Some(state) = self.sniffed.as_mut() {
            state.message = if failed == 0 {
                format!("已将 {ok} 项加入下载中心。")
            } else {
                format!("已加入 {ok} 项，失败 {failed} 项。")
            };
        }
    }

    fn delete_dialog(&mut self, colors: Palette, cx: &mut Context<Self>) -> AnyElement {
        let count = self.delete_targets.len();
        let title = if count == 1 {
            "删除任务".to_string()
        } else {
            format!("删除 {count} 个任务")
        };
        self.modal(colors, cx, "delete-overlay", move |_, colors, cx| {
            div()
                .w(px(440.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(16.))
                .rounded(px(12.))
                .bg(rgb(colors.panel))
                .border_1()
                .border_color(rgb(colors.line))
                .child(div().text_size(px(18.)).child(title))
                .child("是否同时删除已下载的文件？「仅删记录」会保留磁盘上的文件。")
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            button("delete-cancel", "取消", colors.raised, colors.text, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.delete_targets.clear();
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            button(
                                "delete-record",
                                "仅删记录",
                                colors.raised,
                                colors.text,
                                true,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.confirm_delete(false);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            button("delete-file", "删除文件", colors.danger, 0xffffff, true)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.confirm_delete(true);
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
                .into_any_element()
        })
    }

    fn modal(
        &mut self,
        colors: Palette,
        cx: &mut Context<Self>,
        id: &'static str,
        body: impl FnOnce(&mut Self, Palette, &mut Context<Self>) -> AnyElement,
    ) -> AnyElement {
        let panel = body(self, colors, cx);
        div()
            .id(id)
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.62))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if this.add_open && id == "add-overlay" {
                        this.add_open = false;
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(panel),
            )
            .into_any_element()
    }
}

fn display_name(task: &DownloadTask) -> String {
    if task.filename.is_empty() {
        task.url.clone()
    } else {
        task.filename.clone()
    }
}

fn count_tasks(tasks: &[DownloadTask]) -> [usize; 4] {
    [
        tasks.len(),
        tasks
            .iter()
            .filter(|task| matches_filter(task, TaskFilter::Active))
            .count(),
        tasks
            .iter()
            .filter(|task| matches_filter(task, TaskFilter::Completed))
            .count(),
        tasks
            .iter()
            .filter(|task| matches_filter(task, TaskFilter::Other))
            .count(),
    ]
}

fn status_color(status: TaskStatus, colors: Palette) -> u32 {
    match status {
        TaskStatus::Completed => colors.success,
        TaskStatus::Failed => colors.danger,
        TaskStatus::Paused | TaskStatus::Canceled => colors.warning,
        TaskStatus::Downloading | TaskStatus::Probing | TaskStatus::Verifying => colors.accent,
        TaskStatus::Queued => colors.muted,
    }
}

fn connection_label(count: u32) -> String {
    if count == 8 {
        format!("{count} 个连接（推荐）")
    } else {
        format!("{count} 个连接")
    }
}

fn active_label(count: i64) -> String {
    if count == 3 {
        format!("{count} 个任务（推荐）")
    } else {
        format!("{count} 个任务")
    }
}

fn retry_label(count: i64) -> String {
    if count == 0 {
        "不重试".to_string()
    } else if count == 4 {
        format!("{count} 次（推荐）")
    } else {
        format!("{count} 次")
    }
}

fn speed_label(kb: i64) -> String {
    if kb == 0 {
        "不限速".to_string()
    } else if kb % 1024 == 0 {
        format!("{} MB/s", kb / 1024)
    } else {
        format!("{kb} KB/s")
    }
}

fn downloadable(item: &SniffedCandidate) -> bool {
    if item.kind.as_deref() != Some("manifest") {
        return true;
    }
    item.ext
        .as_deref()
        .unwrap_or("")
        .eq_ignore_ascii_case("m3u8")
}

fn candidate_meta(item: &SniffedCandidate) -> String {
    let mut parts = Vec::new();
    if let Some(kind) = item.kind.as_deref() {
        parts.push(
            match kind {
                "image" => "图片",
                "video" => "视频",
                "audio" => "音频",
                "manifest" => "流媒体",
                _ => "媒体",
            }
            .to_string(),
        );
    }
    if let Some(ext) = &item.ext {
        parts.push(ext.to_uppercase());
    }
    if let (Some(width), Some(height)) = (item.width, item.height) {
        if width > 0 && height > 0 {
            parts.push(format!("{width}×{height}"));
        }
    }
    if let Some(bytes) = item.bytes {
        if bytes > 0 {
            parts.push(format_bytes(bytes));
        }
    }
    if parts.is_empty() {
        item.url.clone()
    } else {
        parts.join(" · ")
    }
}

fn check_box(
    id: impl Into<open_gpui::ElementId>,
    on: bool,
    colors: Palette,
) -> open_gpui::Stateful<open_gpui::Div> {
    div()
        .id(id)
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(14.))
                .h(px(14.))
                .rounded(px(3.))
                .border_1()
                .border_color(rgb(if on { colors.accent } else { colors.line }))
                .bg(rgb(if on { colors.accent } else { colors.deep }))
                .child(if on { "✓" } else { "" }),
        )
}

fn menu_item(
    id: &'static str,
    label: &'static str,
    colors: Palette,
) -> open_gpui::Stateful<open_gpui::Div> {
    button(id, label, colors.panel, colors.text, true).w_full()
}

fn flag(
    view: &mut AppView,
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    on: bool,
    colors: Palette,
    cx: &mut Context<AppView>,
) -> AnyElement {
    let _ = view;
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(check_box(id, on, colors).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                match id {
                    "clip" => this.set_clipboard = !this.set_clipboard,
                    "tray" => this.set_tray = !this.set_tray,
                    "hls" => this.set_hls = !this.set_hls,
                    "proxy" => this.set_proxy = !this.set_proxy,
                    _ => {}
                }
                cx.stop_propagation();
                cx.notify();
            }),
        ))
        .child(
            div().flex().flex_col().child(title).child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(detail),
            ),
        )
        .into_any_element()
}

fn section_button(
    _view: &mut AppView,
    id: &'static str,
    label: &'static str,
    section: SettingsSection,
    current: SettingsSection,
    colors: Palette,
    cx: &mut Context<AppView>,
) -> AnyElement {
    let active = section == current;
    button(
        id,
        label,
        if active { colors.accent } else { colors.raised },
        if active { 0xffffff } else { colors.text },
        true,
    )
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, _, _, cx| {
            this.settings_section = section;
            this.open_menu = None;
            cx.notify();
        }),
    )
    .into_any_element()
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let launch_url = std::env::args().skip(1).find(|arg| is_http_url(arg));
    if !shell::acquire_single_instance() {
        if !shell::handoff_to_running(launch_url.as_deref()) {
            eprintln!("RDM 已在运行，但没有连上本地桥（127.0.0.1:43721）。");
            std::process::exit(1);
        }
        return;
    }
    let session = match session::bootstrap(launch_url) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("RDM 启动失败：{error}");
            std::process::exit(1);
        }
    };
    let tray_tx = session.event_tx.clone();
    application().run(move |cx| {
        shell::install_tray(tray_tx);
        cx.bind_keys([
            KeyBinding::new("ctrl-n", NewDownload, None),
            KeyBinding::new("ctrl-q", QuitApp, None),
            KeyBinding::new("ctrl-1", ShowDownloads, None),
            KeyBinding::new("ctrl-2", ShowMedia, None),
            KeyBinding::new("escape", Dismiss, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1120.), px(740.)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(880.), px(560.))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| AppView::new(session, window, cx)),
        ) {
            log::error!("无法打开窗口：{error}");
        }
        cx.activate(true);
    });
}
