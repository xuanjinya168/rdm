//! 下载管理器及其调度器。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;

use rdm_domain::config::AppSettings;
use rdm_domain::{DownloadTask, TaskStatus};
use rdm_engine::{
    postprocess, DownloadEngine, EngineHandle, FinalizeMode, PostProcess, RateLimiter,
    UpdateCallback,
};
use rdm_http::{build_client, ProviderRegistry, ProxyConfig};
use rdm_storage::DownloadDatabase;

use crate::error::ServiceError;

/// 每次进度更新都会通知，传入任务快照与速度（字节/秒）。
pub type ManagerListener = Arc<dyn Fn(DownloadTask, f64) + Send + Sync>;

const SCHEDULER_TICK: Duration = Duration::from_millis(200);

struct ManagerState {
    tasks: HashMap<String, DownloadTask>,
    engines: HashMap<String, EngineHandle>,
    settings: AppSettings,
    listeners: Vec<ManagerListener>,
    stopping: bool,
}

struct ManagerInner {
    database: Arc<DownloadDatabase>,
    limiter: Arc<RateLimiter>,
    providers: Arc<ProviderRegistry>,
    state: Mutex<ManagerState>,
    /// 任务集或设置变更时唤醒调度器。
    wakeup: Notify,
    /// HLS 后处理工具链，由 [`DownloadManager::with_ffmpeg`] 在后台探测后填充。
    /// 未填充（或 ffmpeg 不可用）时，HLS 下载保留原始裸流。
    media: OnceLock<MediaTools>,
}

/// 探测好的媒体工具链：ffmpeg 路径（确认可用）+ 可用的硬件编码器。
struct MediaTools {
    ffmpeg: Option<PathBuf>,
    hw_encoder: Option<String>,
}

/// 持有下载队列并在活跃数上限内调度工作的管理器。
#[derive(Clone)]
pub struct DownloadManager {
    inner: Arc<ManagerInner>,
}

impl DownloadManager {
    /// 在 `database`/`settings` 上构建管理器，恢复持久化的任务并启动后台调度器。
    /// 必须在 tokio 运行时内调用。
    pub fn new(
        database: Arc<DownloadDatabase>,
        settings: AppSettings,
        providers: Arc<ProviderRegistry>,
    ) -> Result<Self, ServiceError> {
        let tasks = database
            .load_tasks()?
            .into_iter()
            .map(|task| (task.id.clone(), task))
            .collect();
        let limiter = Arc::new(RateLimiter::new(settings.speed_limit_bytes.max(0) as u64));
        let inner = Arc::new(ManagerInner {
            database,
            limiter,
            providers,
            state: Mutex::new(ManagerState {
                tasks,
                engines: HashMap::new(),
                settings,
                listeners: Vec::new(),
                stopping: false,
            }),
            wakeup: Notify::new(),
            media: OnceLock::new(),
        });
        let scheduler_inner = Arc::clone(&inner);
        tokio::spawn(async move { scheduler(scheduler_inner).await });
        Ok(Self { inner })
    }

    /// 配置 HLS 后处理用的 ffmpeg。`candidate` 为打包的 sidecar 路径（可选）；
    /// 后台会校验其可用性，失败则回退到 PATH 上的 `ffmpeg`，并探测硬件编码器。
    /// 全部不可用时后处理自动禁用（HLS 下载保留裸流）。不阻塞启动。
    pub fn with_ffmpeg(self, candidate: Option<PathBuf>) -> Self {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let _ = inner.media.set(resolve_media_tools(candidate).await);
        });
        self
    }

    /// 注册进度监听器。
    pub fn add_listener(&self, listener: ManagerListener) {
        self.inner.state().listeners.push(listener);
    }

    /// 全部任务，按创建时间降序排列。
    pub fn all_tasks(&self) -> Vec<DownloadTask> {
        let mut tasks: Vec<DownloadTask> = self.inner.state().tasks.values().cloned().collect();
        tasks.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        tasks
    }

    /// 单个任务的快照（若存在）。
    pub fn get_task(&self, task_id: &str) -> Option<DownloadTask> {
        self.inner.state().tasks.get(task_id).cloned()
    }

    /// 当前应用程序设置。
    pub fn settings(&self) -> AppSettings {
        self.inner.state().settings.clone()
    }

    /// 排队一个新下载。
    pub fn add_download(
        &self,
        url: &str,
        destination: &Path,
        connections: Option<u32>,
        filename: &str,
        expected_sha256: &str,
    ) -> Result<DownloadTask, ServiceError> {
        self.add_download_with_referrer(
            url,
            destination,
            connections,
            filename,
            expected_sha256,
            "",
        )
    }

    /// 排队一个带来源页的下载。来源页会作为 Referer 传给下载 Provider。
    pub fn add_download_with_referrer(
        &self,
        url: &str,
        destination: &Path,
        connections: Option<u32>,
        filename: &str,
        expected_sha256: &str,
        referrer: &str,
    ) -> Result<DownloadTask, ServiceError> {
        let task = {
            let state = self.inner.state();
            let connections =
                connections.unwrap_or(state.settings.default_connections.max(1) as u32);
            let mut task = DownloadTask::create(
                url,
                destination,
                connections,
                filename.trim(),
                expected_sha256,
            )?;
            let referrer = referrer.trim();
            if !referrer.is_empty() {
                task.referrer = Some(referrer.to_string());
            }
            task
        };
        {
            let mut state = self.inner.state();
            if state.stopping {
                return Err(ServiceError::ShuttingDown);
            }
            self.inner.database.save_task(&task)?;
            state.tasks.insert(task.id.clone(), task.clone());
        }
        self.inner.wakeup.notify_one();
        self.inner.emit(&task, 0.0);
        Ok(task)
    }

    /// 重新排队任务（已完成或已在运行时为空操作）。
    pub fn start(&self, task_id: &str) -> Result<(), ServiceError> {
        let snapshot = {
            let mut state = self.inner.state();
            if state.engines.contains_key(task_id) {
                return Ok(());
            }
            let task = state
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| ServiceError::NotFound(task_id.to_string()))?;
            if task.status == TaskStatus::Completed {
                return Ok(());
            }
            task.status = TaskStatus::Queued;
            task.error = None;
            task.updated_at = unix_time();
            let snapshot = task.clone();
            self.inner.database.save_task(&snapshot)?;
            snapshot
        };
        self.inner.wakeup.notify_one();
        self.inner.emit(&snapshot, 0.0);
        Ok(())
    }

    /// 暂停正在运行的下载，或将仍在排队的任务标记为暂停。
    pub fn pause(&self, task_id: &str) -> Result<(), ServiceError> {
        let mut snapshot: Option<DownloadTask> = None;
        let engine = {
            let mut state = self.inner.state();
            match state.engines.get(task_id) {
                Some(engine) => Some(engine.clone()),
                None => {
                    let task = state
                        .tasks
                        .get_mut(task_id)
                        .ok_or_else(|| ServiceError::NotFound(task_id.to_string()))?;
                    if task.status == TaskStatus::Queued {
                        task.status = TaskStatus::Paused;
                        task.updated_at = unix_time();
                        snapshot = Some(task.clone());
                        self.inner.database.save_task(snapshot.as_ref().unwrap())?;
                    }
                    None
                }
            }
        };
        if let Some(engine) = engine {
            engine.pause();
        } else if let Some(snapshot) = snapshot {
            self.inner.wakeup.notify_one();
            self.inner.emit(&snapshot, 0.0);
        }
        Ok(())
    }

    /// 取消下载（无论运行中还是排队中）。
    pub fn cancel(&self, task_id: &str) -> Result<(), ServiceError> {
        let mut snapshot: Option<DownloadTask> = None;
        let engine = {
            let mut state = self.inner.state();
            match state.engines.get(task_id) {
                Some(engine) => Some(engine.clone()),
                None => {
                    let task = state
                        .tasks
                        .get_mut(task_id)
                        .ok_or_else(|| ServiceError::NotFound(task_id.to_string()))?;
                    task.status = TaskStatus::Canceled;
                    task.updated_at = unix_time();
                    snapshot = Some(task.clone());
                    self.inner.database.save_task(snapshot.as_ref().unwrap())?;
                    None
                }
            }
        };
        if let Some(engine) = engine {
            engine.cancel();
        } else if let Some(snapshot) = snapshot {
            self.inner.wakeup.notify_one();
            self.inner.emit(&snapshot, 0.0);
        }
        Ok(())
    }

    /// 移除任务；当前正在下载时返回 false。若 `delete_file` 为 true，
    /// 同时删除其输出文件和 `.part` 文件。
    pub fn delete(&self, task_id: &str, delete_file: bool) -> Result<bool, ServiceError> {
        let mut state = self.inner.state();
        if state.engines.contains_key(task_id) {
            return Ok(false);
        }
        let task = state
            .tasks
            .get(task_id)
            .ok_or_else(|| ServiceError::NotFound(task_id.to_string()))?;
        if delete_file && !task.filename.is_empty() {
            for path in [task.output_path(), task.part_path()] {
                if path.is_file() {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        self.inner.database.delete_task(task_id)?;
        state.tasks.remove(task_id);
        Ok(true)
    }

    /// 替换设置并实时应用新的速度限制。
    pub fn update_settings(&self, settings: AppSettings) {
        {
            let mut state = self.inner.state();
            self.inner
                .limiter
                .set_rate(settings.speed_limit_bytes.max(0) as u64);
            state.settings = settings;
        }
        self.inner.wakeup.notify_one();
    }

    /// 停止调度，暂停所有运行中的下载并等待它们结束。幂等操作。
    pub async fn shutdown(&self) {
        {
            let mut state = self.inner.state();
            if state.stopping {
                return;
            }
            state.stopping = true;
            for engine in state.engines.values() {
                engine.pause();
            }
        }
        self.inner.wakeup.notify_one();
        loop {
            if self.inner.state().engines.is_empty() {
                break;
            }
            tokio::select! {
                _ = self.inner.wakeup.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }
}

impl ManagerInner {
    fn state(&self) -> MutexGuard<'_, ManagerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit(&self, task: &DownloadTask, speed: f64) {
        let listeners = self.state().listeners.clone();
        for listener in listeners {
            listener(task.clone(), speed);
        }
    }

    /// 构建引擎进度回调：将每个快照折叠到任务表中，
    /// 任务离开活跃集时唤醒调度器，并扇出给监听器。
    fn make_callback(self: &Arc<Self>) -> UpdateCallback {
        let inner = Arc::clone(self);
        Arc::new(move |snapshot: DownloadTask, speed: f64| {
            {
                let mut state = inner.state();
                if let Some(task) = state.tasks.get_mut(&snapshot.id) {
                    *task = snapshot.clone();
                }
            }
            if !snapshot.status.is_active() {
                inner.wakeup.notify_one();
            }
            inner.emit(&snapshot, speed);
        })
    }

    /// 根据当前设置为一次下载解析 HLS 后处理配置。
    /// 工具链未就绪或 ffmpeg 不可用时返回 `None`（保留裸流）。
    fn build_postprocess(&self, transcode: bool) -> Option<PostProcess> {
        let tools = self.media.get()?;
        let ffmpeg = tools.ffmpeg.clone()?;
        let mode = match (transcode, &tools.hw_encoder) {
            // 仅当用户开启转码且实测到可用 GPU 编码器时才转码，否则无损封装。
            (true, Some(encoder)) => FinalizeMode::Transcode {
                video_encoder: encoder.clone(),
            },
            _ => FinalizeMode::Remux,
        };
        Some(PostProcess { ffmpeg, mode })
    }

    /// 为 `task` 派生一个引擎，跟踪其句柄并在完成时回收。
    fn launch(self: &Arc<Self>, task: DownloadTask) {
        let (retry_count, connections, proxy, transcode) = {
            let state = self.state();
            (
                state.settings.retry_count.max(0) as u32,
                task.connections,
                proxy_from_settings(&state.settings),
                state.settings.hls_transcode,
            )
        };
        let client = build_client(connections, &proxy).unwrap_or_default();
        let engine = DownloadEngine::new(
            task.clone(),
            Arc::clone(&self.database),
            Arc::clone(&self.limiter),
            Arc::clone(&self.providers),
            client,
            self.make_callback(),
            retry_count,
        )
        .with_postprocess(self.build_postprocess(transcode));
        let handle = engine.handle();
        self.state().engines.insert(task.id.clone(), handle);

        let inner = Arc::clone(self);
        let task_id = task.id.clone();
        tokio::spawn(async move {
            let final_task = engine.run().await;
            {
                let mut state = inner.state();
                state.engines.remove(&task_id);
                if let Some(task) = state.tasks.get_mut(&final_task.id) {
                    *task = final_task;
                }
            }
            inner.wakeup.notify_one();
        });
    }
}

/// 解析 HLS 后处理工具链：优先用打包的 `candidate`，其次用 PATH 上的
/// `ffmpeg`；二者都不可用则禁用后处理。可用时进一步实测硬件编码器。
async fn resolve_media_tools(candidate: Option<PathBuf>) -> MediaTools {
    let mut ffmpeg = None;
    if let Some(path) = candidate {
        if postprocess::probe_available(&path).await {
            ffmpeg = Some(path);
        }
    }
    if ffmpeg.is_none() {
        let on_path = PathBuf::from("ffmpeg");
        if postprocess::probe_available(&on_path).await {
            ffmpeg = Some(on_path);
        }
    }
    let hw_encoder = match &ffmpeg {
        Some(path) => postprocess::detect_hw_encoder(path).await,
        None => None,
    };
    match (&ffmpeg, &hw_encoder) {
        (None, _) => log::info!("未找到可用的 ffmpeg，HLS 下载将保留原始裸流"),
        (Some(_), Some(encoder)) => log::info!("HLS 后处理就绪，检测到硬件编码器 {encoder}"),
        (Some(_), None) => log::info!("HLS 后处理就绪（仅封装，未检测到可用 GPU 编码器）"),
    }
    MediaTools { ffmpeg, hw_encoder }
}

/// 将 [`AppSettings`] 的代理部分转换为 [`ProxyConfig`]。
///
/// 代理未启用时返回未激活的配置，使客户端回退到 reqwest 的默认值。
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

async fn scheduler(inner: Arc<ManagerInner>) {
    loop {
        let to_launch = {
            let state = inner.state();
            if state.stopping {
                break;
            }
            let available = state.settings.max_active_downloads.max(0) as usize;
            let free = available.saturating_sub(state.engines.len());
            if free == 0 {
                Vec::new()
            } else {
                let mut queued: Vec<DownloadTask> = state
                    .tasks
                    .values()
                    .filter(|task| {
                        task.status == TaskStatus::Queued && !state.engines.contains_key(&task.id)
                    })
                    .cloned()
                    .collect();
                queued.sort_by(|a, b| a.created_at.total_cmp(&b.created_at));
                queued.truncate(free);
                queued
            }
        };
        for task in to_launch {
            inner.launch(task);
        }
        tokio::select! {
            _ = inner.wakeup.notified() => {}
            _ = tokio::time::sleep(SCHEDULER_TICK) => {}
        }
    }
}

fn unix_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn start_server(data: Arc<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let data = Arc::clone(&data);
                tokio::spawn(async move {
                    let _ = handle(&mut socket, &data).await;
                });
            }
        });
        format!("http://{addr}")
    }

    async fn handle(socket: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = socket.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&buffer);
        let mut lines = text.lines();
        let request_line = lines.next().unwrap_or_default();
        let method = request_line.split_whitespace().next().unwrap_or("GET");
        let range = lines
            .clone()
            .find_map(|l| l.strip_prefix("Range:").map(|v| v.trim().to_string()));

        let total = data.len();
        let (mut start, mut end, mut partial) = (0usize, total.saturating_sub(1), false);
        if let Some(spec) = range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
            let mut bounds = spec.splitn(2, '-');
            start = bounds.next().unwrap_or("").parse().unwrap_or(0);
            let last = bounds.next().unwrap_or("");
            if !last.is_empty() {
                end = last
                    .parse::<usize>()
                    .unwrap_or(end)
                    .min(total.saturating_sub(1));
            }
            partial = true;
        }
        let body = &data[start..=end];
        let mut headers = String::new();
        headers.push_str(if partial {
            "HTTP/1.1 206 Partial Content\r\n"
        } else {
            "HTTP/1.1 200 OK\r\n"
        });
        headers.push_str("Content-Disposition: attachment; filename=\"fixture.bin\"\r\n");
        headers.push_str(&format!("Content-Length: {}\r\n", body.len()));
        headers.push_str("ETag: \"v1\"\r\nAccept-Ranges: bytes\r\n");
        if partial {
            headers.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
        }
        headers.push_str("Connection: close\r\n\r\n");
        socket.write_all(headers.as_bytes()).await?;
        if method != "HEAD" {
            socket.write_all(body).await?;
        }
        Ok(())
    }

    fn manager(dir: &Path) -> DownloadManager {
        // 为回环测试服务器绕过任何主机 HTTP_PROXY。
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        let database = Arc::new(DownloadDatabase::open(Some(dir.join("downloads.db"))).unwrap());
        let settings = AppSettings {
            download_dir: dir.to_string_lossy().into_owned(),
            ..AppSettings::default()
        };
        DownloadManager::new(database, settings, Arc::new(ProviderRegistry::default())).unwrap()
    }

    /// 一个永不自动开始下载的管理器（`max_active_downloads = 0`），
    /// 便于在没有网络的情况下确定性地驱动排队任务的生命周期。
    fn idle_manager(dir: &Path) -> DownloadManager {
        let database = Arc::new(DownloadDatabase::open(Some(dir.join("d.db"))).unwrap());
        let settings = AppSettings {
            download_dir: dir.to_string_lossy().into_owned(),
            max_active_downloads: 0,
            ..AppSettings::default()
        };
        DownloadManager::new(database, settings, Arc::new(ProviderRegistry::default())).unwrap()
    }

    async fn wait_for_status(
        manager: &DownloadManager,
        task_id: &str,
        status: TaskStatus,
    ) -> DownloadTask {
        for _ in 0..500 {
            if let Some(task) = manager.get_task(task_id) {
                if task.status == status {
                    return task;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("task did not reach {status:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn add_download_schedules_and_completes() {
        let data = Arc::new(
            (0..900 * 1024)
                .map(|i| (i % 251) as u8)
                .collect::<Vec<u8>>(),
        );
        let base = start_server(Arc::clone(&data)).await;
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path());

        let updates = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&updates);
        manager.add_listener(Arc::new(move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        let task = manager
            .add_download(&format!("{base}/file.bin"), dir.path(), Some(4), "", "")
            .unwrap();
        let done = wait_for_status(&manager, &task.id, TaskStatus::Completed).await;

        let output = dir.path().join(&done.filename);
        assert_eq!(std::fs::read(&output).unwrap(), *data);
        assert!(updates.load(Ordering::SeqCst) > 0);
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pause_keeps_queued_task_out_of_scheduling() {
        let dir = tempfile::tempdir().unwrap();
        let manager = idle_manager(dir.path());

        let task = manager
            .add_download("http://127.0.0.1:1/never.bin", dir.path(), Some(1), "", "")
            .unwrap();
        manager.pause(&task.id).unwrap();
        let paused = wait_for_status(&manager, &task.id, TaskStatus::Paused).await;
        assert_eq!(paused.status, TaskStatus::Paused);
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_removes_task_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let manager = idle_manager(dir.path());
        let task = manager
            .add_download(
                "http://127.0.0.1:1/x.bin",
                dir.path(),
                Some(1),
                "keep.bin",
                "",
            )
            .unwrap();
        manager.pause(&task.id).unwrap();
        wait_for_status(&manager, &task.id, TaskStatus::Paused).await;

        std::fs::write(dir.path().join("keep.bin"), b"data").unwrap();
        assert!(manager.delete(&task.id, true).unwrap());
        assert!(manager.get_task(&task.id).is_none());
        assert!(!dir.path().join("keep.bin").exists());
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path());
        manager.shutdown().await;
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn add_download_does_not_leak_into_memory_when_persistence_fails() {
        let dir = tempfile::tempdir().unwrap();
        let database =
            Arc::new(DownloadDatabase::open(Some(dir.path().join("closed.db"))).unwrap());
        let manager = DownloadManager::new(
            Arc::clone(&database),
            AppSettings {
                max_active_downloads: 0,
                ..AppSettings::default()
            },
            Arc::new(ProviderRegistry::default()),
        )
        .unwrap();
        database.close();

        let result =
            manager.add_download("https://example.test/file.bin", dir.path(), Some(1), "", "");

        assert!(matches!(result, Err(ServiceError::Store(_))));
        assert!(manager.all_tasks().is_empty());
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_surfaces_database_load_failures() {
        let dir = tempfile::tempdir().unwrap();
        let database =
            Arc::new(DownloadDatabase::open(Some(dir.path().join("closed.db"))).unwrap());
        database.close();

        let result = DownloadManager::new(
            database,
            AppSettings::default(),
            Arc::new(ProviderRegistry::default()),
        );

        assert!(matches!(result, Err(ServiceError::Store(_))));
    }
}
