//! 分段下载引擎。使用 tokio 异步运行时驱动多连接下载。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, CONTENT_LENGTH, CONTENT_RANGE, IF_RANGE, RANGE};
use sha2::{Digest, Sha256};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use rdm_domain::segments::{build_segments, valid_resume_segments};
use rdm_domain::validation::sanitize_filename;
use rdm_domain::{DownloadTask, Segment, TaskStatus};
use rdm_http::{
    parse_content_range, probe_url, ContentRange, PreparedDownload, ProbeResult, ProviderRegistry,
};
use rdm_storage::DownloadDatabase;

use crate::error::EngineError;
use crate::files::{publish_part_file, reserve_part_file};
use crate::hls::{self, ByteRange, MediaPlaylist, Playlist};
use crate::rate_limit::RateLimiter;
use crate::sparse::mark_sparse;

const MIN_SEGMENT_SIZE: u64 = 512 * 1024;
/// 繁忙分段仅在剩余至少 `2 * MIN_SPLIT_SIZE` 时才会被切分，因此两半
/// 都值得一次请求，且切分落地时网络中已有的块（远小于一半）不会
/// 跨越新的末端。
const MIN_SPLIT_SIZE: u64 = 1024 * 1024;

// 速度样本按 50 ms 分桶折叠，窗口为 3 s。
const SPEED_BUCKETS_PER_SECOND: u64 = 20;
const SPEED_WINDOW_BUCKETS: u64 = 3 * SPEED_BUCKETS_PER_SECOND;

const UI_NOTIFY_INTERVAL: Duration = Duration::from_millis(200);
const DB_SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// 每次受限的进度更新都会调用，传入任务快照与当前速度（字节/秒）。
pub type UpdateCallback = Arc<dyn Fn(DownloadTask, f64) + Send + Sync>;

/// 暂停/取消/失败信号，在引擎与其工作器之间共享。
struct Signals {
    pause: AtomicBool,
    cancel: AtomicBool,
    failure: AtomicBool,
    /// 任意信号触发时取消，以中断阻塞的读取。
    token: CancellationToken,
}

impl Signals {
    fn new() -> Self {
        Self {
            pause: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            failure: AtomicBool::new(false),
            token: CancellationToken::new(),
        }
    }

    fn request_pause(&self) {
        self.pause.store(true, Ordering::SeqCst);
        self.token.cancel();
    }

    fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.token.cancel();
    }

    fn set_failure(&self) {
        self.failure.store(true, Ordering::SeqCst);
        self.token.cancel();
    }

    fn is_canceled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn is_paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    fn should_abort(&self) -> bool {
        self.is_paused() || self.is_canceled() || self.failure.load(Ordering::SeqCst)
    }
}

/// 从外部暂停或取消正在运行的引擎的句柄。
#[derive(Clone)]
pub struct EngineHandle {
    signals: Arc<Signals>,
}

impl EngineHandle {
    /// 请求暂停；运行将以 `Paused` 状态结束。
    pub fn pause(&self) {
        self.signals.request_pause();
    }

    /// 请求取消；运行将以 `Canceled` 状态结束。
    pub fn cancel(&self) {
        self.signals.request_cancel();
    }
}

/// 工作器之间共享的可变下载状态，由一个互斥锁保护。
struct EngineState {
    task: DownloadTask,
    segments: Vec<Segment>,
    pending: VecDeque<u32>,
    active: HashSet<u32>,
    resume_segments: Vec<Segment>,
    start: Instant,
    samples: VecDeque<(u64, u64)>,
    samples_total: u64,
}

impl EngineState {
    fn new(task: DownloadTask) -> Self {
        Self {
            task,
            segments: Vec::new(),
            pending: VecDeque::new(),
            active: HashSet::new(),
            resume_segments: Vec::new(),
            start: Instant::now(),
            samples: VecDeque::new(),
            samples_total: 0,
        }
    }

    fn record_sample(&mut self, amount: u64) {
        let bucket = (self.start.elapsed().as_secs_f64() * SPEED_BUCKETS_PER_SECOND as f64) as u64;
        match self.samples.back_mut() {
            Some(last) if last.0 == bucket => last.1 += amount,
            _ => self.samples.push_back((bucket, amount)),
        }
        self.samples_total += amount;
        let cutoff = bucket.saturating_sub(SPEED_WINDOW_BUCKETS);
        while let Some(front) = self.samples.front() {
            if front.0 < cutoff {
                self.samples_total -= self.samples.pop_front().unwrap().1;
            } else {
                break;
            }
        }
    }

    fn current_speed(&self) -> f64 {
        let Some(front) = self.samples.front() else {
            return 0.0;
        };
        let now = self.start.elapsed().as_secs_f64() * SPEED_BUCKETS_PER_SECOND as f64;
        let elapsed = (now - front.0 as f64).max(1.0) / SPEED_BUCKETS_PER_SECOND as f64;
        self.samples_total as f64 / elapsed
    }

    /// 窃取最繁忙活动分段的后半部分，返回 `(受害者, 新建)` 索引。
    /// 受害者继续流式传输其裁剪后的范围，因此不会重复写入任何字节。
    /// 调用方持有状态锁。
    fn split_largest_active(&mut self) -> Option<(u32, u32)> {
        if !(self.task.supports_ranges && self.task.total_size.unwrap_or(0) > 0) {
            return None;
        }
        let mut victim: Option<u32> = None;
        let mut victim_remaining = 0u64;
        for segment in &self.segments {
            if !self.active.contains(&segment.index) {
                continue;
            }
            let Some(end) = segment.end else { continue };
            let next_byte = segment.next_byte();
            if end < next_byte {
                continue;
            }
            let remaining = end - next_byte + 1;
            if remaining > victim_remaining {
                victim_remaining = remaining;
                victim = Some(segment.index);
            }
        }
        let victim_index = victim?;
        if victim_remaining < 2 * MIN_SPLIT_SIZE {
            return None;
        }
        let split_at = self.segments[victim_index as usize].next_byte() + victim_remaining / 2;
        let created_index = self.segments.len() as u32;
        let created = Segment {
            task_id: self.task.id.clone(),
            index: created_index,
            start: split_at,
            end: self.segments[victim_index as usize].end,
            downloaded: 0,
        };
        self.segments[victim_index as usize].end = Some(split_at - 1);
        self.segments.push(created);
        self.active.insert(created_index);
        Some((victim_index, created_index))
    }
}

struct NotifyState {
    last_ui: Instant,
    last_db: Instant,
}

/// 在请求前快照的每分段值，因此在网络读取期间不持有锁。
struct SegmentRequest {
    next_byte: u64,
    end: Option<u64>,
    total_size: Option<u64>,
    expects_partial: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    part_path: PathBuf,
}

/// 一个已配置、准备运行的单任务下载。
pub struct DownloadEngine {
    task: DownloadTask,
    database: Arc<DownloadDatabase>,
    limiter: Arc<RateLimiter>,
    providers: Arc<ProviderRegistry>,
    client: reqwest::Client,
    callback: UpdateCallback,
    retry_count: u32,
    signals: Arc<Signals>,
}

impl DownloadEngine {
    /// 创建引擎。`client` 应按任务的连接数配置（例如通过 `rdm_http::build_client`）。
    pub fn new(
        task: DownloadTask,
        database: Arc<DownloadDatabase>,
        limiter: Arc<RateLimiter>,
        providers: Arc<ProviderRegistry>,
        client: reqwest::Client,
        callback: UpdateCallback,
        retry_count: u32,
    ) -> Self {
        Self {
            task,
            database,
            limiter,
            providers,
            client,
            callback,
            retry_count,
            signals: Arc::new(Signals::new()),
        }
    }

    /// 运行时暂停/取消此引擎的句柄。
    pub fn handle(&self) -> EngineHandle {
        EngineHandle {
            signals: Arc::clone(&self.signals),
        }
    }

    /// 运行下载直至完成，返回最终任务。错误通过任务的状态/错误字段报告，
    /// 不会 panic。
    pub async fn run(self) -> DownloadTask {
        let prepared = match self.providers.prepare(&self.task) {
            Ok(prepared) => prepared,
            Err(error) => return self.fail_immediately(error.to_string()),
        };
        let prepared_headers = match prepared.request_headers() {
            Ok(headers) => headers,
            Err(error) => return self.fail_immediately(error.to_string()),
        };
        let inner = Arc::new(EngineInner {
            database: self.database,
            limiter: self.limiter,
            client: self.client,
            callback: self.callback,
            retry_count: self.retry_count,
            signals: self.signals,
            prepared_headers,
            download_url: Mutex::new(prepared.url.clone()),
            state: Mutex::new(EngineState::new(self.task)),
            notify: Mutex::new(NotifyState {
                last_ui: Instant::now() - DB_SAVE_INTERVAL,
                last_db: Instant::now() - DB_SAVE_INTERVAL,
            }),
        });

        let result = if hls::is_hls_url(&prepared.url) {
            inner.execute_hls(&prepared).await
        } else {
            inner.execute_direct(&prepared).await
        };

        match result {
            Ok(()) => {}
            Err(EngineError::Interrupted) => {
                inner.finalize_status(inner.aborted_status(), None);
            }
            Err(error) => {
                if inner.signals.is_paused() || inner.signals.is_canceled() {
                    inner.finalize_status(inner.aborted_status(), None);
                } else {
                    inner.finalize_status(TaskStatus::Failed, Some(error.to_string()));
                }
            }
        }
        let final_task = inner.state().task.clone();
        final_task
    }

    fn fail_immediately(self, error: String) -> DownloadTask {
        let mut task = self.task;
        task.status = TaskStatus::Failed;
        task.updated_at = unix_time();
        task.error = Some(error.clone());
        if let Err(persist_error) = self.database.save_task(&task) {
            log::error!("Could not persist immediate task failure: {persist_error}");
            task.error = Some(format!(
                "{error}; additionally failed to persist state: {persist_error}"
            ));
        }
        (self.callback)(task.clone(), 0.0);
        task
    }
}

struct EngineInner {
    database: Arc<DownloadDatabase>,
    limiter: Arc<RateLimiter>,
    client: reqwest::Client,
    callback: UpdateCallback,
    retry_count: u32,
    signals: Arc<Signals>,
    prepared_headers: HeaderMap,
    download_url: Mutex<String>,
    state: Mutex<EngineState>,
    notify: Mutex<NotifyState>,
}

impl EngineInner {
    fn state(&self) -> MutexGuard<'_, EngineState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn download_url(&self) -> String {
        self.download_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn aborted_status(&self) -> TaskStatus {
        if self.signals.is_canceled() {
            TaskStatus::Canceled
        } else {
            TaskStatus::Paused
        }
    }

    fn check_interrupted(&self) -> Result<(), EngineError> {
        if self.signals.should_abort() {
            Err(EngineError::Interrupted)
        } else {
            Ok(())
        }
    }

    async fn execute_direct(
        self: &Arc<Self>,
        prepared: &PreparedDownload,
    ) -> Result<(), EngineError> {
        self.set_status(TaskStatus::Probing, None)?;
        let probe = probe_url(&self.client, &prepared.url, &self.prepared_headers).await?;
        *self
            .download_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = probe.final_url.clone();
        self.prepare_task(&probe)?;
        self.prepare_segments()?;
        self.check_interrupted()?;
        self.set_status(TaskStatus::Downloading, None)?;
        self.download_segments().await?;
        if self.signals.is_canceled() {
            self.set_status(TaskStatus::Canceled, None)?;
        } else if self.signals.is_paused() {
            self.set_status(TaskStatus::Paused, None)?;
        } else {
            self.finish().await?;
        }
        Ok(())
    }

    async fn execute_hls(self: &Arc<Self>, prepared: &PreparedDownload) -> Result<(), EngineError> {
        self.set_status(TaskStatus::Probing, None)?;
        let (media_url, mut media) = self.resolve_hls_media_playlist(&prepared.url).await?;
        *self
            .download_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = media_url.clone();

        let concurrency = self.hls_concurrency(media.segments.len());
        // 并发 HEAD 探测分片大小，得到精确的总字节数用于进度估算。
        // 探测失败的分片 size 保持 None，total_size() 随之为 None（退化为不定长下载）。
        hls::probe_segment_sizes(
            &self.client,
            &self.prepared_headers,
            &mut media,
            concurrency,
        )
        .await;
        let total_size = media.total_size();
        let fmp4 = media.init_segment.is_some();
        let requested_filename = {
            let st = self.state();
            (!st.task.filename.is_empty()).then(|| st.task.filename.clone())
        };

        self.prepare_hls_task(&media_url, total_size, fmp4, requested_filename)?;
        self.check_interrupted()?;
        self.set_status(TaskStatus::Downloading, None)?;

        let temp_dir = self.hls_temp_dir();
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir)?;
        }
        std::fs::create_dir_all(&temp_dir)?;

        let result = async {
            let keys = self.fetch_hls_keys(&media).await?;
            let actual_bytes = self
                .download_hls_segments_to_cache(&media, &keys, &temp_dir, total_size)
                .await?;
            let concatenated_bytes = self.concatenate_hls_segments(&media, &temp_dir)?;
            if concatenated_bytes != actual_bytes {
                return Err(EngineError::Download(format!(
                    "HLS concatenated size mismatch ({concatenated_bytes}/{actual_bytes} bytes)"
                )));
            }
            Ok(actual_bytes)
        }
        .await;

        match result {
            Ok(actual_bytes) => {
                std::fs::remove_dir_all(&temp_dir)?;
                self.finish_hls(actual_bytes).await?;
            }
            Err(error) => {
                if let Err(cleanup_error) = std::fs::remove_dir_all(&temp_dir) {
                    log::warn!(
                        "Could not remove HLS temporary directory {}: {cleanup_error}",
                        temp_dir.display()
                    );
                }
                let (task_id, part_path) = {
                    let st = self.state();
                    (st.task.id.clone(), st.task.part_path())
                };
                let _ = self.discard_partial(&task_id, &part_path);
                return Err(error);
            }
        }
        Ok(())
    }

    async fn resolve_hls_media_playlist(
        &self,
        initial_url: &str,
    ) -> Result<(String, MediaPlaylist), EngineError> {
        let mut url = initial_url.to_string();
        for _ in 0..5 {
            self.check_interrupted()?;
            let content = self.fetch_hls_text(&url).await?;
            match hls::parse_playlist(&content, &url)? {
                Playlist::Media(media) => return Ok((url, media)),
                Playlist::Master(master) => {
                    let variant = master.best_variant().ok_or_else(|| {
                        EngineError::Download("HLS master playlist has no variants".into())
                    })?;
                    url = variant.uri.clone();
                }
            }
        }
        Err(EngineError::Download(
            "HLS master playlist nesting is too deep".into(),
        ))
    }

    async fn fetch_hls_text(&self, url: &str) -> Result<String, EngineError> {
        let mut builder = self.client.get(url);
        builder = builder.headers(self.prepared_headers.clone());
        let response = self.send_interruptible(builder).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(EngineError::Download(format!(
                "HTTP status {} while fetching HLS playlist",
                status.as_u16()
            )));
        }
        let bytes = self.read_hls_body(response, false).await?;
        String::from_utf8(bytes)
            .map_err(|_| EngineError::Download("HLS playlist is not valid UTF-8".into()))
    }

    fn prepare_hls_task(
        &self,
        playlist_url: &str,
        total_size: Option<u64>,
        fmp4: bool,
        requested_filename: Option<String>,
    ) -> Result<(), EngineError> {
        let (task_id, destination) = {
            let st = self.state();
            (st.task.id.clone(), st.task.destination.clone())
        };
        self.database.save_segments(&task_id, &[])?;
        let requested = hls_output_filename(playlist_url, requested_filename, fmp4);
        let (name, _) = reserve_part_file(Path::new(&destination), &requested)?;
        {
            let mut st = self.state();
            st.task.filename = name;
            st.task.total_size = total_size;
            st.task.downloaded = 0;
            st.task.supports_ranges = false;
            st.task.etag = None;
            st.task.last_modified = None;
            st.task.actual_sha256 = None;
            st.task.error = None;
        }
        self.notify(true)?;
        Ok(())
    }

    fn hls_temp_dir(&self) -> PathBuf {
        let st = self.state();
        Path::new(&st.task.destination).join(format!(".rdm-{}-hls", st.task.id))
    }

    async fn fetch_hls_keys(
        &self,
        media: &MediaPlaylist,
    ) -> Result<HashMap<String, [u8; 16]>, EngineError> {
        let mut keys = HashMap::new();
        for segment in &media.segments {
            let Some(encryption) = &segment.encryption else {
                continue;
            };
            if keys.contains_key(&encryption.uri) {
                continue;
            }
            self.check_interrupted()?;
            let mut builder = self.client.get(&encryption.uri);
            builder = builder.headers(self.prepared_headers.clone());
            let response = self.send_interruptible(builder).await?;
            let status = response.status();
            if !status.is_success() {
                return Err(EngineError::Download(format!(
                    "HTTP status {} while fetching HLS AES-128 key",
                    status.as_u16()
                )));
            }
            let bytes = self.read_hls_body(response, false).await?;
            if bytes.len() != 16 {
                return Err(hls::HlsError::InvalidKeyLength(bytes.len()).into());
            }
            let mut key = [0u8; 16];
            key.copy_from_slice(&bytes);
            keys.insert(encryption.uri.clone(), key);
        }
        Ok(keys)
    }

    async fn download_hls_segments_to_cache(
        &self,
        media: &MediaPlaylist,
        keys: &HashMap<String, [u8; 16]>,
        temp_dir: &Path,
        total_size: Option<u64>,
    ) -> Result<u64, EngineError> {
        let mut actual_bytes = 0u64;
        let mut downloaded_bytes = 0u64;

        // fMP4：先下载初始化分片（#EXT-X-MAP），它必须前置拼接才能播放。
        // 初始化分片按 HLS 规范不加密。
        if let Some(init) = &media.init_segment {
            self.check_interrupted()?;
            let init_bytes = self
                .download_hls_segment(None, &init.uri, init.byte_range)
                .await?;
            std::fs::write(hls_init_path(temp_dir), &init_bytes)?;
            let len = init_bytes.len() as u64;
            actual_bytes = actual_bytes.saturating_add(len);
            downloaded_bytes = downloaded_bytes.saturating_add(len);
            {
                let mut st = self.state();
                st.task.total_size = total_size;
                st.task.downloaded = downloaded_bytes;
            }
            self.notify(true)?;
        }

        // 顺序下载媒体分片：分片之间相互独立，但为了保证 EngineState
        // （std::sync::Mutex）在受限线程数的运行时下不与下载 worker 互相阻塞，
        // 这里串行下载、逐片更新进度。每片解密后写入按 index 命名的临时文件，
        // 最后由 concatenate_hls_segments 按顺序拼接（init 在最前）。
        for segment in &media.segments {
            self.check_interrupted()?;
            let encrypted_or_plain = self
                .download_hls_segment(Some(segment.index), &segment.uri, segment.byte_range)
                .await?;
            let decoded = if let Some(encryption) = &segment.encryption {
                let key = keys.get(&encryption.uri).ok_or_else(|| {
                    EngineError::Download(format!("Missing HLS AES-128 key {}", encryption.uri))
                })?;
                let iv = segment
                    .iv()
                    .ok_or_else(|| EngineError::Download("Missing HLS AES-128 IV".to_string()))?;
                hls::decrypt_aes128_cbc(&encrypted_or_plain, key, &iv)?
            } else {
                encrypted_or_plain
            };
            std::fs::write(hls_segment_path(temp_dir, segment.index), &decoded)?;
            let len = decoded.len() as u64;
            actual_bytes = actual_bytes.saturating_add(len);
            downloaded_bytes = downloaded_bytes.saturating_add(len);
            {
                let mut st = self.state();
                st.task.total_size = total_size;
                st.task.downloaded = downloaded_bytes;
            }
            self.notify(true)?;
        }
        Ok(actual_bytes)
    }

    /// HLS 分片下载并发度：受 connections 限制，但不超过分片数，
    /// 且至少为 1。
    fn hls_concurrency(&self, segment_count: usize) -> usize {
        let connections = self.state().task.connections as usize;
        connections.min(segment_count.max(1)).max(1)
    }

    /// 下载单个媒体分片（`index` 为 1 基序号的来源）或 init 段（`index` 为 None），
    /// 失败时按 `retry_count` 重试。`byte_range` 存在时以 HTTP Range 仅请求该区间。
    async fn download_hls_segment(
        &self,
        index: Option<usize>,
        uri: &str,
        byte_range: Option<ByteRange>,
    ) -> Result<Vec<u8>, EngineError> {
        let mut attempts = 0u32;
        loop {
            self.check_interrupted()?;
            let result = self.download_hls_segment_once(uri, byte_range).await;
            match result {
                Ok(bytes) => return Ok(bytes),
                Err(EngineError::Interrupted) => return Err(EngineError::Interrupted),
                Err(error) => {
                    if self.signals.should_abort() {
                        return Err(EngineError::Interrupted);
                    }
                    if !error.is_retryable() || attempts >= self.retry_count {
                        let label = match index {
                            Some(index) => format!("HLS segment {}", index + 1),
                            None => "HLS init segment".to_string(),
                        };
                        return Err(EngineError::Download(format!("{label} failed: {error}")));
                    }
                    attempts += 1;
                    self.interruptible_sleep(2u64.pow(attempts - 1).min(8))
                        .await?;
                }
            }
        }
    }

    async fn download_hls_segment_once(
        &self,
        uri: &str,
        byte_range: Option<ByteRange>,
    ) -> Result<Vec<u8>, EngineError> {
        let mut builder = self.client.get(uri);
        builder = builder.headers(self.prepared_headers.clone());
        if let Some(range) = byte_range {
            builder = builder.header(RANGE, range.http_range_value());
        }
        let response = self.send_interruptible(builder).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(EngineError::Download(format!(
                "HTTP status {} while fetching HLS segment",
                status.as_u16()
            )));
        }
        let body = self.read_hls_body(response, true).await?;
        // 请求了区间但服务器以 200 返回全量时，自行裁剪，避免拼接出错误内容。
        if let Some(range) = byte_range {
            if status == reqwest::StatusCode::OK {
                return Ok(range.slice(&body).to_vec());
            }
        }
        Ok(body)
    }

    async fn read_hls_body(
        &self,
        response: reqwest::Response,
        count_speed: bool,
    ) -> Result<Vec<u8>, EngineError> {
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        loop {
            let item = tokio::select! {
                biased;
                _ = self.signals.token.cancelled() => return Err(EngineError::Interrupted),
                item = stream.next() => item,
            };
            let chunk = match item {
                Some(Ok(chunk)) => chunk,
                Some(Err(error)) => return Err(EngineError::Reqwest(error)),
                None => break,
            };
            self.check_interrupted()?;
            if chunk.is_empty() {
                continue;
            }
            let amount = chunk.len() as u64;
            let signals = Arc::clone(&self.signals);
            if !self
                .limiter
                .acquire(amount, move || signals.should_abort())
                .await
            {
                return Err(EngineError::Interrupted);
            }
            if count_speed {
                self.state().record_sample(amount);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    fn concatenate_hls_segments(
        &self,
        media: &MediaPlaylist,
        temp_dir: &Path,
    ) -> Result<u64, EngineError> {
        let part_path = self.state().task.part_path();
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&part_path)?;
        let mut total = 0u64;
        // fMP4：初始化分片必须前置拼接，否则拼接出的分片无法解复用播放。
        if media.init_segment.is_some() {
            self.check_interrupted()?;
            let init_path = hls_init_path(temp_dir);
            let bytes = std::fs::read(&init_path)?;
            output.write_all(&bytes)?;
            total = total.saturating_add(bytes.len() as u64);
        }
        for segment in &media.segments {
            self.check_interrupted()?;
            let segment_path = hls_segment_path(temp_dir, segment.index);
            let bytes = std::fs::read(&segment_path)?;
            output.write_all(&bytes)?;
            total = total.saturating_add(bytes.len() as u64);
        }
        Ok(total)
    }

    async fn finish_hls(&self, actual: u64) -> Result<(), EngineError> {
        let (part_path, expected_sha, task_id) = {
            let st = self.state();
            (
                st.task.part_path(),
                st.task.expected_sha256.clone(),
                st.task.id.clone(),
            )
        };
        if !part_path.exists() {
            return Err(EngineError::Download(
                "Temporary HLS download file is missing".into(),
            ));
        }
        if let Some(expected_sha) = expected_sha {
            self.set_status(TaskStatus::Verifying, None)?;
            let actual_sha = self.calculate_sha256(&part_path).await?;
            self.state().task.actual_sha256 = Some(actual_sha.clone());
            if actual_sha != expected_sha {
                self.discard_partial(&task_id, &part_path)?;
                return Err(EngineError::Download(format!(
                    "SHA-256 verification failed (expected {expected_sha}, got {actual_sha})"
                )));
            }
        }

        let mut task = self.state().task.clone();
        publish_part_file(&mut task)?;
        {
            let mut st = self.state();
            st.task.filename = task.filename;
            st.task.downloaded = actual;
            st.task.total_size = Some(actual);
        }
        self.database.save_segments(&task_id, &[])?;
        self.set_status(TaskStatus::Completed, None)?;
        Ok(())
    }

    /// 决定续传还是重新下载，预留输出名并更新元数据。
    fn prepare_task(&self, probe: &ProbeResult) -> Result<(), EngineError> {
        let (old_size, old_etag, old_modified, task_id, destination, had_filename, filename) = {
            let st = self.state();
            (
                st.task.total_size,
                st.task.etag.clone(),
                st.task.last_modified.clone(),
                st.task.id.clone(),
                st.task.destination.clone(),
                !st.task.filename.is_empty(),
                st.task.filename.clone(),
            )
        };
        let metadata_changed = (old_size.is_some() && old_size != probe.total_size)
            || (old_etag.is_some() && old_etag.as_deref() != probe.etag.as_deref())
            || (old_modified.is_some()
                && old_modified.as_deref() != probe.last_modified.as_deref());

        let existing = self.database.load_segments(&task_id)?;
        {
            let mut st = self.state();
            st.task.total_size = probe.total_size;
            st.task.supports_ranges = probe.supports_ranges;
            st.task.etag = probe.etag.clone();
            st.task.last_modified = probe.last_modified.clone();
            st.task.actual_sha256 = None;
            st.task.filename = if had_filename {
                filename_with_probe_extension(&filename, &probe.filename)
            } else {
                sanitize_filename(&probe.filename)
            };
        }

        let (output_path, part_path) = {
            let st = self.state();
            (st.task.output_path(), st.task.part_path())
        };
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let part_exists = part_path.exists();
        let can_resume = probe.supports_ranges
            && !output_path.exists()
            && part_exists
            && !existing.is_empty()
            && !metadata_changed
            && valid_resume_segments(
                &existing,
                probe.total_size,
                std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0),
            );

        if can_resume {
            self.state().resume_segments = existing;
        } else {
            self.database.save_segments(&task_id, &[])?;
            self.state().task.downloaded = 0;
            if part_exists && !existing.is_empty() {
                let _ = std::fs::remove_file(&part_path);
            }
            let current_filename = self.state().task.filename.clone();
            let (name, _) = reserve_part_file(Path::new(&destination), &current_filename)?;
            let mut st = self.state();
            st.task.filename = name;
            st.resume_segments = Vec::new();
        }
        self.state().task.error = None;
        self.notify(true)?;
        Ok(())
    }

    /// 构建（或恢复）分段列表并准备 `.part` 文件。
    fn prepare_segments(&self) -> Result<(), EngineError> {
        let resume = std::mem::take(&mut self.state().resume_segments);
        if !resume.is_empty() {
            let downloaded: u64 = resume.iter().map(|s| s.downloaded).sum();
            let mut st = self.state();
            st.task.downloaded = downloaded;
            st.segments = resume;
            drop(st);
            self.notify(true)?;
            return Ok(());
        }

        let (task_id, total, supports, connections, part_path) = {
            let st = self.state();
            (
                st.task.id.clone(),
                st.task.total_size,
                st.task.supports_ranges,
                st.task.connections,
                st.task.part_path(),
            )
        };
        let segments = build_segments(&task_id, total, supports, connections, MIN_SEGMENT_SIZE);
        self.state().task.downloaded = 0;
        self.database.save_segments(&task_id, &segments)?;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&part_path)?;
        file.set_len(0)?;
        if let Some(total) = total {
            if total > 0 {
                mark_sparse(&file);
                file.set_len(total)?;
            }
        }
        drop(file);

        self.state().segments = segments;
        self.notify(true)?;
        Ok(())
    }

    async fn download_segments(self: &Arc<Self>) -> Result<(), EngineError> {
        {
            let mut st = self.state();
            let pending: VecDeque<u32> = st
                .segments
                .iter()
                .filter(|s| !s.complete())
                .map(|s| s.index)
                .collect();
            if pending.is_empty() {
                return Ok(());
            }
            st.pending = pending;
            st.active.clear();
        }

        let workers = self.worker_count();
        let mut set = JoinSet::new();
        for _ in 0..workers {
            let me = Arc::clone(self);
            set.spawn(async move { me.segment_worker().await });
        }

        let mut first_error: Option<EngineError> = None;
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(())) | Ok(Err(EngineError::Interrupted)) => {}
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                Err(join_error) => {
                    first_error.get_or_insert_with(|| {
                        EngineError::Download(format!("worker task failed: {join_error}"))
                    });
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn worker_count(&self) -> u32 {
        let st = self.state();
        let pending: Vec<&Segment> = st.segments.iter().filter(|s| !s.complete()).collect();
        if st.task.supports_ranges && st.task.total_size.unwrap_or(0) > 0 {
            let remaining: u64 = pending
                .iter()
                .map(|s| s.size().unwrap_or(0) - s.downloaded)
                .sum();
            // 工作器数多于待处理分段是有用的：空闲工作器立即切分繁忙分段，
            // 因此只有一个大段残留的续传下载仍可并行化。
            let ideal = (pending.len() as u64).max(remaining / MIN_SPLIT_SIZE);
            (st.task.connections as u64).min(ideal).max(1) as u32
        } else {
            1
        }
    }

    async fn segment_worker(&self) -> Result<(), EngineError> {
        loop {
            self.check_interrupted()?;
            let index = match self.next_segment()? {
                Some(index) => index,
                None => return Ok(()),
            };
            let outcome = self.download_segment(index).await;
            self.state().active.remove(&index);
            if let Err(error) = outcome {
                if !matches!(error, EngineError::Interrupted) {
                    self.signals.set_failure();
                }
                return Err(error);
            }
        }
    }

    fn next_segment(&self) -> Result<Option<u32>, EngineError> {
        let mut st = self.state();
        while let Some(index) = st.pending.pop_front() {
            if st.segments[index as usize].complete() {
                continue;
            }
            st.active.insert(index);
            return Ok(Some(index));
        }
        let Some((victim_index, created_index)) = st.split_largest_active() else {
            return Ok(None);
        };
        // 在锁内持久化：同一受害者的两次快速切分必须按应用顺序提交，
        // 否则崩溃可能为续传留下重叠范围。
        let victim = st.segments[victim_index as usize].clone();
        let created = st.segments[created_index as usize].clone();
        self.database.split_segment(&victim, &created)?;
        Ok(Some(created_index))
    }

    async fn download_segment(&self, index: u32) -> Result<(), EngineError> {
        let mut attempts = 0u32;
        loop {
            self.check_interrupted()?;
            let result = self.download_segment_once(index).await;
            let segment = self.state().segments[index as usize].clone();
            self.database.update_segment(&segment)?;
            match result {
                Ok(()) => return Ok(()),
                Err(EngineError::Interrupted) => return Err(EngineError::Interrupted),
                Err(error) => {
                    // 暂停/取消会让任何进行中的 I/O 错误变为中断，
                    // 而非下载失败。
                    if self.signals.should_abort() {
                        return Err(EngineError::Interrupted);
                    }
                    if !error.is_retryable() {
                        return Err(error);
                    }
                    if attempts >= self.retry_count {
                        return Err(EngineError::Download(format!(
                            "Segment {} failed: {error}",
                            index + 1
                        )));
                    }
                    attempts += 1;
                    self.interruptible_sleep(2u64.pow(attempts - 1).min(8))
                        .await?;
                }
            }
        }
    }

    async fn download_segment_once(&self, index: u32) -> Result<(), EngineError> {
        let request = {
            let st = self.state();
            let segment = &st.segments[index as usize];
            SegmentRequest {
                next_byte: segment.next_byte(),
                end: segment.end,
                total_size: st.task.total_size,
                expects_partial: st.task.supports_ranges,
                etag: st.task.etag.clone(),
                last_modified: st.task.last_modified.clone(),
                part_path: st.task.part_path(),
            }
        };
        if let Some(end) = request.end {
            if request.next_byte > end {
                return Ok(());
            }
        }

        let mut builder = self.client.get(self.download_url());
        builder = builder.headers(self.prepared_headers.clone());
        if request.expects_partial {
            let end = request.end.map(|e| e.to_string()).unwrap_or_default();
            builder = builder.header(RANGE, format!("bytes={}-{}", request.next_byte, end));
            if let Some(etag) = &request.etag {
                builder = builder.header(IF_RANGE, etag.as_str());
            } else if let Some(modified) = &request.last_modified {
                builder = builder.header(IF_RANGE, modified.as_str());
            }
        }

        let response = self.send_interruptible(builder).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(EngineError::Download(format!(
                "HTTP status {}",
                status.as_u16()
            )));
        }
        if request.expects_partial && status.as_u16() != 206 {
            return Err(EngineError::Download(
                "Server stopped honoring byte-range requests".into(),
            ));
        }
        let response_range_len = if request.expects_partial {
            Some(validate_partial_response(&response, &request)?)
        } else {
            None
        };
        if !request.expects_partial && request.next_byte != 0 {
            return Err(EngineError::Download(
                "This server does not support resuming".into(),
            ));
        }

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&request.part_path)?;
        file.seek(SeekFrom::Start(request.next_byte))?;

        let mut stream = response.bytes_stream();
        let mut checkpoint = Instant::now();
        let mut response_bytes = 0u64;
        loop {
            let item = tokio::select! {
                biased;
                _ = self.signals.token.cancelled() => return Err(EngineError::Interrupted),
                item = stream.next() => item,
            };
            let chunk = match item {
                Some(Ok(chunk)) => chunk,
                Some(Err(error)) => return Err(EngineError::Reqwest(error)),
                None => break,
            };
            self.check_interrupted()?;
            if chunk.is_empty() {
                continue;
            }
            response_bytes = response_bytes.saturating_add(chunk.len() as u64);
            if response_range_len.is_some_and(|expected| response_bytes > expected) {
                return Err(EngineError::Download(format!(
                    "Ranged response body exceeded Content-Range length ({response_bytes} bytes)"
                )));
            }

            let chunk = self.trim_chunk(index, chunk);
            if chunk.is_empty() {
                break;
            }
            let amount = chunk.len() as u64;
            let signals = Arc::clone(&self.signals);
            if !self
                .limiter
                .acquire(amount, move || signals.should_abort())
                .await
            {
                return Err(EngineError::Interrupted);
            }
            file.write_all(&chunk)?;
            {
                let mut st = self.state();
                st.segments[index as usize].downloaded += amount;
                st.task.downloaded += amount;
                st.record_sample(amount);
            }
            if checkpoint.elapsed() >= DB_SAVE_INTERVAL {
                let segment = self.state().segments[index as usize].clone();
                if let Err(error) = self.database.update_segment(&segment) {
                    log::warn!(
                        "Could not persist progress for task {} segment {}: {error}",
                        segment.task_id,
                        segment.index
                    );
                }
                checkpoint = Instant::now();
            }
            let _ = self.notify(false);
        }

        let (downloaded, size) = {
            let st = self.state();
            let segment = &st.segments[index as usize];
            (segment.downloaded, segment.size())
        };
        let current_end = self.state().segments[index as usize].end;
        if current_end == request.end
            && response_range_len.is_some_and(|expected| response_bytes != expected)
        {
            return Err(EngineError::Download(format!(
                "Ranged response body length mismatch ({response_bytes}/{} bytes)",
                response_range_len.unwrap_or_default()
            )));
        }
        if let Some(size) = size {
            if downloaded != size {
                return Err(EngineError::Download(format!(
                    "Segment ended early ({downloaded}/{size} bytes)"
                )));
            }
        }
        Ok(())
    }

    /// 根据分段当前末端裁剪刚读取的块，并发切分可能已将其缩小。
    /// 无剩余时返回空。
    fn trim_chunk(&self, index: u32, chunk: Bytes) -> Bytes {
        let st = self.state();
        let segment = &st.segments[index as usize];
        let Some(end) = segment.end else {
            return chunk;
        };
        let next_byte = segment.next_byte();
        if end < next_byte {
            return Bytes::new();
        }
        let remaining = end - next_byte + 1;
        if (chunk.len() as u64) > remaining {
            chunk.slice(0..remaining as usize)
        } else {
            chunk
        }
    }

    async fn send_interruptible(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, EngineError> {
        tokio::select! {
            biased;
            _ = self.signals.token.cancelled() => Err(EngineError::Interrupted),
            result = builder.send() => Ok(result?),
        }
    }

    async fn interruptible_sleep(&self, seconds: u64) -> Result<(), EngineError> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < deadline {
            self.check_interrupted()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            tokio::time::sleep(remaining.min(Duration::from_millis(100))).await;
        }
        Ok(())
    }

    async fn finish(&self) -> Result<(), EngineError> {
        let (expected, actual, part_path, expected_sha, task_id) = {
            let st = self.state();
            let actual: u64 = st.segments.iter().map(|s| s.downloaded).sum();
            (
                st.task.total_size,
                actual,
                st.task.part_path(),
                st.task.expected_sha256.clone(),
                st.task.id.clone(),
            )
        };
        if let Some(expected) = expected {
            if actual != expected {
                return Err(EngineError::Download(format!(
                    "Size verification failed ({actual}/{expected} bytes)"
                )));
            }
        }
        if !part_path.exists() {
            return Err(EngineError::Download(
                "Temporary download file is missing".into(),
            ));
        }
        if let Some(expected_sha) = expected_sha {
            self.set_status(TaskStatus::Verifying, None)?;
            let actual_sha = self.calculate_sha256(&part_path).await?;
            self.state().task.actual_sha256 = Some(actual_sha.clone());
            if actual_sha != expected_sha {
                // 字节已完整但错误；丢弃它们，以便重试从头开始，
                // 而非重新哈希同一损坏文件。
                self.discard_partial(&task_id, &part_path)?;
                return Err(EngineError::Download(format!(
                    "SHA-256 verification failed (expected {expected_sha}, got {actual_sha})"
                )));
            }
        }

        let mut task = self.state().task.clone();
        publish_part_file(&mut task)?;
        {
            let mut st = self.state();
            st.task.filename = task.filename;
            st.task.downloaded = actual;
        }
        self.set_status(TaskStatus::Completed, None)?;
        Ok(())
    }

    fn discard_partial(&self, task_id: &str, part_path: &Path) -> Result<(), EngineError> {
        self.database.save_segments(task_id, &[])?;
        self.state().task.downloaded = 0;
        let _ = std::fs::remove_file(part_path);
        Ok(())
    }

    /// 在异步运行时外哈希完成的文件：读取和摘要多 GB 文件是纯阻塞工作，
    /// 因此它在阻塞池上运行，而非阻塞 tokio 工作线程。
    /// 无锁读取是安全的，因为每个分段工作器已在 `finish` 时加入。
    async fn calculate_sha256(&self, path: &Path) -> Result<String, EngineError> {
        let path = path.to_path_buf();
        let signals = Arc::clone(&self.signals);
        tokio::task::spawn_blocking(move || -> Result<String, EngineError> {
            let mut file = std::fs::File::open(&path)?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0u8; 1024 * 1024];
            loop {
                if signals.should_abort() {
                    return Err(EngineError::Interrupted);
                }
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(to_hex(&hasher.finalize()))
        })
        .await
        .map_err(|error| EngineError::Download(format!("hashing task failed: {error}")))?
    }

    fn set_status(&self, status: TaskStatus, error: Option<String>) -> Result<(), EngineError> {
        {
            let mut st = self.state();
            st.task.status = status;
            st.task.error = error;
        }
        self.notify(true)
    }

    fn finalize_status(&self, status: TaskStatus, error: Option<String>) {
        if let Err(persist_error) = self.set_status(status, error.clone()) {
            log::error!("Could not persist terminal task status: {persist_error}");
            let message = match error {
                Some(error) => {
                    format!("{error}; additionally failed to persist state: {persist_error}")
                }
                None => format!("Failed to persist task state: {persist_error}"),
            };
            let snapshot = {
                let mut st = self.state();
                st.task.status = TaskStatus::Failed;
                st.task.error = Some(message);
                st.task.updated_at = unix_time();
                st.task.clone()
            };
            (self.callback)(snapshot, 0.0);
        }
    }

    /// 受限的进度扇出：最多每秒持久化一次任务，最多每 200 ms 调用一次
    /// 回调（强制时立即）。
    fn notify(&self, force: bool) -> Result<(), EngineError> {
        let now = Instant::now();
        let save_to_db;
        {
            let mut notify = self
                .notify
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !force && now.duration_since(notify.last_ui) < UI_NOTIFY_INTERVAL {
                return Ok(());
            }
            notify.last_ui = now;
            save_to_db = force || now.duration_since(notify.last_db) >= DB_SAVE_INTERVAL;
            if save_to_db {
                notify.last_db = now;
            }
        }
        let (snapshot, speed) = {
            let mut st = self.state();
            st.task.updated_at = unix_time();
            (st.task.clone(), st.current_speed())
        };
        if save_to_db {
            if let Err(error) = self.database.save_task(&snapshot) {
                if force {
                    return Err(error.into());
                }
                log::warn!(
                    "Could not persist progress for task {}: {error}",
                    snapshot.id
                );
            }
        }
        (self.callback)(snapshot, speed);
        Ok(())
    }
}

fn validate_partial_response(
    response: &reqwest::Response,
    request: &SegmentRequest,
) -> Result<u64, EngineError> {
    let value = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| EngineError::Download("206 response omitted Content-Range".into()))?;
    let ContentRange::Bytes { start, end, total } = parse_content_range(value)
        .ok_or_else(|| EngineError::Download(format!("Invalid Content-Range: {value}")))?
    else {
        return Err(EngineError::Download(format!(
            "Unexpected unsatisfied Content-Range in 206 response: {value}"
        )));
    };
    if start != request.next_byte {
        return Err(EngineError::Download(format!(
            "Content-Range starts at {start}, expected {}",
            request.next_byte
        )));
    }
    if request.end.is_some_and(|requested_end| end > requested_end) {
        return Err(EngineError::Download(format!(
            "Content-Range ends at {end}, beyond requested end {}",
            request.end.unwrap_or_default()
        )));
    }
    if let Some(expected_total) = request.total_size {
        if total != Some(expected_total) {
            return Err(EngineError::Download(format!(
                "Content-Range total {:?} does not match expected size {expected_total}",
                total
            )));
        }
    }
    let range_len = end - start + 1;
    if let Some(content_length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        if content_length != range_len {
            return Err(EngineError::Download(format!(
                "Content-Length {content_length} does not match Content-Range length {range_len}"
            )));
        }
    }
    Ok(range_len)
}

fn hls_output_filename(playlist_url: &str, requested: Option<String>, fmp4: bool) -> String {
    let base = match requested {
        Some(filename) => sanitize_filename(&filename),
        None => {
            let from_url = Url::parse(playlist_url)
                .ok()
                .and_then(|parsed| {
                    parsed.path_segments().and_then(|mut segments| {
                        segments
                            .rfind(|segment| !segment.is_empty())
                            .map(str::to_string)
                    })
                })
                .unwrap_or_else(|| "download".to_string());
            sanitize_filename(&from_url)
        }
    };
    with_hls_extension(base, fmp4)
}

/// 把 HLS 输出文件名强制为合适的容器扩展名：
/// fMP4/CMAF → `.m4s`，MPEG-TS → `.ts`。
fn with_hls_extension(filename: String, fmp4: bool) -> String {
    let target = if fmp4 { "m4s" } else { "ts" };
    if let Some(ext) = file_extension(&filename) {
        let stem_len = filename.len().saturating_sub(ext.len() + 1);
        return format!("{}.{target}", &filename[..stem_len]);
    }
    append_extension(filename, target)
}

fn hls_segment_path(temp_dir: &Path, index: usize) -> PathBuf {
    // 扩展名仅用于临时文件区分，拼接后统一由 with_hls_extension 决定输出名。
    temp_dir.join(format!("{index:08}.seg"))
}

/// fMP4 初始化分片（#EXT-X-MAP）的临时缓存路径；拼接时位于最前。
fn hls_init_path(temp_dir: &Path) -> PathBuf {
    temp_dir.join("init.seg")
}

fn file_extension(filename: &str) -> Option<&str> {
    let (_, ext) = filename.rsplit_once('.')?;
    if ext.is_empty()
        || ext.len() > 12
        || !ext.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    Some(ext)
}

fn append_extension(filename: String, ext: &str) -> String {
    let suffix = format!(".{ext}");
    let max_base = 240usize.saturating_sub(suffix.chars().count());
    let base = filename.chars().take(max_base).collect::<String>();
    format!("{base}{suffix}")
}

fn filename_with_probe_extension(requested: &str, probed: &str) -> String {
    let filename = sanitize_filename(requested);
    if file_extension(&filename).is_some() {
        return filename;
    }
    match file_extension(probed) {
        Some(ext) => append_extension(filename, ext),
        None => filename,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        hex.push(HEX[(byte >> 4) as usize] as char);
        hex.push(HEX[(byte & 0x0f) as usize] as char);
    }
    hex
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
    use aes::Aes128;
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use rdm_http::{DownloadProvider, HttpError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    type Aes128CbcEncryptor = cbc::Encryptor<Aes128>;

    fn make_data(size: usize) -> Vec<u8> {
        (0..size).map(|i| (i % 251) as u8).collect()
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        to_hex(&hasher.finalize())
    }

    #[test]
    fn explicit_filename_without_extension_reuses_probed_extension() {
        assert_eq!(
            filename_with_probe_extension("custom name", "download.jpg"),
            "custom name.jpg"
        );
        assert_eq!(
            filename_with_probe_extension("already.png", "download.jpg"),
            "already.png"
        );
        assert_eq!(
            filename_with_probe_extension("bad/name", "download.mp4"),
            "bad_name.mp4"
        );
        assert_eq!(
            filename_with_probe_extension("custom", "download"),
            "custom"
        );
    }

    async fn start_server(data: Arc<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let data = Arc::clone(&data);
                tokio::spawn(async move {
                    let _ = handle_connection(&mut socket, &data).await;
                });
            }
        });
        format!("http://{addr}")
    }

    struct HlsFixtureResponse {
        body: Vec<u8>,
        content_type: &'static str,
        content_length: bool,
    }

    impl HlsFixtureResponse {
        fn text(body: impl Into<String>) -> Self {
            Self {
                body: body.into().into_bytes(),
                content_type: "application/vnd.apple.mpegurl",
                content_length: true,
            }
        }

        fn binary(body: Vec<u8>, content_length: bool) -> Self {
            Self {
                body,
                content_type: "application/octet-stream",
                content_length,
            }
        }
    }

    async fn start_hls_server(
        routes: std::collections::HashMap<String, HlsFixtureResponse>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let routes = Arc::new(routes);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let routes = Arc::clone(&routes);
                tokio::spawn(async move {
                    let _ = handle_hls_connection(&mut socket, &routes).await;
                });
            }
        });
        format!("http://{addr}")
    }

    async fn handle_hls_connection(
        socket: &mut TcpStream,
        routes: &std::collections::HashMap<String, HlsFixtureResponse>,
    ) -> std::io::Result<()> {
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
            if buffer.len() > 64 * 1024 {
                return Ok(());
            }
        }
        let text = String::from_utf8_lossy(&buffer);
        let request_line = text.lines().next().unwrap_or_default();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap_or("GET");
        let raw_path = request_parts.next().unwrap_or("/");
        let path = raw_path.split('?').next().unwrap_or(raw_path);

        let Some(response) = routes.get(path) else {
            socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            return Ok(());
        };

        // 服务端兑现 Range 请求（206 + 区间切片），让 byte-range 分片下载被真实验证。
        if let Some((start, end)) = parse_range_header(&text, response.body.len()) {
            let slice = response.body.get(start..=end).unwrap_or(&[]);
            let mut headers = String::from("HTTP/1.1 206 Partial Content\r\n");
            headers.push_str(&format!("Content-Type: {}\r\n", response.content_type));
            headers.push_str(&format!("Content-Length: {}\r\n", slice.len()));
            headers.push_str(&format!(
                "Content-Range: bytes {start}-{end}/{}\r\n",
                response.body.len()
            ));
            headers.push_str("Connection: close\r\n\r\n");
            socket.write_all(headers.as_bytes()).await?;
            if method != "HEAD" {
                socket.write_all(slice).await?;
            }
            return Ok(());
        }

        let mut headers = String::from("HTTP/1.1 200 OK\r\n");
        headers.push_str(&format!("Content-Type: {}\r\n", response.content_type));
        if response.content_length {
            headers.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
        }
        headers.push_str("Connection: close\r\n\r\n");
        socket.write_all(headers.as_bytes()).await?;
        if method != "HEAD" {
            socket.write_all(&response.body).await?;
        }
        Ok(())
    }

    /// 解析 `Range: bytes=start-end` 头，返回含两端的字节下标；缺失或非法时为 None。
    fn parse_range_header(request: &str, body_len: usize) -> Option<(usize, usize)> {
        let value = request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("range")
                .then(|| value.trim())
        })?;
        let spec = value.strip_prefix("bytes=")?;
        let (start, end) = spec.split_once('-')?;
        let start: usize = start.trim().parse().ok()?;
        let end = end.trim();
        let end = if end.is_empty() {
            body_len.saturating_sub(1)
        } else {
            end.parse().ok()?
        };
        let end = end.min(body_len.saturating_sub(1));
        if start > end {
            return None;
        }
        Some((start, end))
    }

    async fn handle_connection(socket: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
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
            if buffer.len() > 64 * 1024 {
                return Ok(());
            }
        }
        let text = String::from_utf8_lossy(&buffer);
        let mut lines = text.lines();
        let request_line = lines.next().unwrap_or_default();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap_or("GET");
        let path = request_parts.next().unwrap_or("/");
        let mut range = None;
        let mut authorization = None;
        for line in lines {
            if let Some(value) = line
                .strip_prefix("Range:")
                .or_else(|| line.strip_prefix("range:"))
            {
                range = Some(value.trim().to_string());
            }
            if let Some(value) = line
                .strip_prefix("Authorization:")
                .or_else(|| line.strip_prefix("authorization:"))
            {
                authorization = Some(value.trim().to_string());
            }
        }
        if path.contains("private") && authorization.as_deref() != Some("Bearer test-token") {
            socket
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            return Ok(());
        }

        let total = data.len();
        let honor_range = !path.contains("no-range");
        let mut start = 0usize;
        let mut end = total.saturating_sub(1);
        let mut partial = false;
        if honor_range {
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
                if start > end {
                    let response = format!(
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    socket.write_all(response.as_bytes()).await?;
                    return Ok(());
                }
                partial = true;
            }
        }

        let body = &data[start..=end];
        let mut headers = String::new();
        headers.push_str(if partial {
            "HTTP/1.1 206 Partial Content\r\n"
        } else {
            "HTTP/1.1 200 OK\r\n"
        });
        headers.push_str("Content-Type: application/octet-stream\r\n");
        headers.push_str("Content-Disposition: attachment; filename=\"fixture.bin\"\r\n");
        if !path.contains("short-body") {
            let content_length = if path.contains("wrong-length") {
                body.len().saturating_add(1)
            } else {
                body.len()
            };
            headers.push_str(&format!("Content-Length: {content_length}\r\n"));
        }
        headers.push_str("ETag: \"v1\"\r\n");
        if honor_range {
            headers.push_str("Accept-Ranges: bytes\r\n");
        }
        if partial && !path.contains("missing-range") {
            let (header_start, header_end) = if path.contains("wrong-range") {
                (start.saturating_add(1), end.saturating_add(1))
            } else {
                (start, end)
            };
            let header_total = if path.contains("wrong-total") && !(start == 0 && end == 0) {
                total.saturating_add(1)
            } else {
                total
            };
            headers.push_str(&format!(
                "Content-Range: bytes {header_start}-{header_end}/{header_total}\r\n"
            ));
        }
        headers.push_str("Connection: close\r\n\r\n");
        socket.write_all(headers.as_bytes()).await?;
        if method != "HEAD" {
            let body = if path.contains("short-body") && !body.is_empty() {
                &body[..body.len() - 1]
            } else {
                body
            };
            socket.write_all(body).await?;
        }
        Ok(())
    }

    fn no_proxy_client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    fn setup(
        dir: &Path,
    ) -> (
        Arc<DownloadDatabase>,
        Arc<RateLimiter>,
        Arc<ProviderRegistry>,
    ) {
        let database = Arc::new(DownloadDatabase::open(Some(dir.join("downloads.db"))).unwrap());
        (
            database,
            Arc::new(RateLimiter::new(0)),
            Arc::new(ProviderRegistry::default()),
        )
    }

    fn encrypt_hls_segment(plain: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
        Aes128CbcEncryptor::new(key.into(), iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plain)
    }

    async fn run_task(url: &str, dir: &Path, connections: u32) -> DownloadTask {
        let (database, limiter, providers) = setup(dir);
        let task = DownloadTask::create(url, dir, connections, "", "").unwrap();
        database.save_task(&task).unwrap();
        let engine = DownloadEngine::new(
            task,
            Arc::clone(&database),
            limiter,
            providers,
            no_proxy_client(),
            Arc::new(|_, _| {}),
            2,
        );
        engine.run().await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn downloads_aes128_hls_without_segment_content_length() {
        let key = [0x31u8; 16];
        let iv = [0x42u8; 16];
        let chunks = [
            b"first clear transport-stream payload".to_vec(),
            b"second payload that is intentionally longer".to_vec(),
            b"third payload".to_vec(),
        ];
        let expected = chunks.concat();
        let encrypted = chunks
            .iter()
            .map(|chunk| encrypt_hls_segment(chunk, &key, &iv))
            .collect::<Vec<_>>();
        let routes = std::collections::HashMap::from([
            (
                "/enc/index.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-TARGETDURATION:8
#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x42424242424242424242424242424242
#EXTINF:8.0,
seg0.ts
#EXTINF:8.0,
seg1.ts
#EXTINF:8.0,
seg2.ts
#EXT-X-ENDLIST
",
                ),
            ),
            (
                "/enc/key.bin".to_string(),
                HlsFixtureResponse::binary(key.to_vec(), true),
            ),
            (
                "/enc/seg0.ts".to_string(),
                HlsFixtureResponse::binary(encrypted[0].clone(), false),
            ),
            (
                "/enc/seg1.ts".to_string(),
                HlsFixtureResponse::binary(encrypted[1].clone(), false),
            ),
            (
                "/enc/seg2.ts".to_string(),
                HlsFixtureResponse::binary(encrypted[2].clone(), false),
            ),
        ]);
        let base = start_hls_server(routes).await;
        let dir = tempfile::tempdir().unwrap();

        let final_task = run_task(&format!("{base}/enc/index.m3u8"), dir.path(), 8).await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        assert_eq!(final_task.filename, "index.ts");
        assert_eq!(final_task.total_size, Some(expected.len() as u64));
        assert_eq!(final_task.downloaded, expected.len() as u64);
        assert_eq!(std::fs::read(final_task.output_path()).unwrap(), expected);
        assert!(!dir
            .path()
            .join(format!(".rdm-{}-hls", final_task.id))
            .exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn downloads_plain_vod_hls() {
        let chunks = [
            b"plain segment zero".to_vec(),
            b"plain segment one".to_vec(),
            b"plain segment two".to_vec(),
        ];
        let expected = chunks.concat();
        let routes = std::collections::HashMap::from([
            (
                "/plain/index.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg0.ts
#EXTINF:4.0,
seg1.ts
#EXTINF:4.0,
seg2.ts
#EXT-X-ENDLIST
",
                ),
            ),
            (
                "/plain/seg0.ts".to_string(),
                HlsFixtureResponse::binary(chunks[0].clone(), false),
            ),
            (
                "/plain/seg1.ts".to_string(),
                HlsFixtureResponse::binary(chunks[1].clone(), false),
            ),
            (
                "/plain/seg2.ts".to_string(),
                HlsFixtureResponse::binary(chunks[2].clone(), false),
            ),
        ]);
        let base = start_hls_server(routes).await;
        let dir = tempfile::tempdir().unwrap();

        let final_task = run_task(&format!("{base}/plain/index.m3u8"), dir.path(), 4).await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        assert_eq!(std::fs::read(final_task.output_path()).unwrap(), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn master_hls_selects_highest_bandwidth_variant() {
        let low = b"low quality bytes".to_vec();
        let high_chunks = [b"high quality ".to_vec(), b"transport stream".to_vec()];
        let expected = high_chunks.concat();
        let routes = std::collections::HashMap::from([
            (
                "/master.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=640x360
low/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=1920x1080
high/index.m3u8
",
                ),
            ),
            (
                "/low/index.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg.ts
#EXT-X-ENDLIST
",
                ),
            ),
            (
                "/low/seg.ts".to_string(),
                HlsFixtureResponse::binary(low, false),
            ),
            (
                "/high/index.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg0.ts
#EXTINF:4.0,
seg1.ts
#EXT-X-ENDLIST
",
                ),
            ),
            (
                "/high/seg0.ts".to_string(),
                HlsFixtureResponse::binary(high_chunks[0].clone(), false),
            ),
            (
                "/high/seg1.ts".to_string(),
                HlsFixtureResponse::binary(high_chunks[1].clone(), false),
            ),
        ]);
        let base = start_hls_server(routes).await;
        let dir = tempfile::tempdir().unwrap();

        let final_task = run_task(&format!("{base}/master.m3u8"), dir.path(), 4).await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        assert_eq!(std::fs::read(final_task.output_path()).unwrap(), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn downloads_fmp4_hls_and_prepends_init_segment() {
        // fMP4/CMAF：#EXT-X-MAP 声明的初始化分片必须前置拼接，
        // 否则拼接出的分片无法解复用播放。
        let init_bytes = b"ftypisom...init payload".to_vec();
        let chunks = [
            b"moof...first mdat".to_vec(),
            b"moof...second mdat".to_vec(),
        ];
        let expected = init_bytes
            .iter()
            .chain(chunks[0].iter())
            .chain(chunks[1].iter())
            .copied()
            .collect::<Vec<_>>();
        let routes = std::collections::HashMap::from([
            (
                "/fmp4/index.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-TARGETDURATION:4
#EXT-X-MAP:URI=\"init.mp4\"
#EXTINF:4.0,
seg0.m4s
#EXTINF:4.0,
seg1.m4s
#EXT-X-ENDLIST
",
                ),
            ),
            (
                "/fmp4/init.mp4".to_string(),
                HlsFixtureResponse::binary(init_bytes.clone(), false),
            ),
            (
                "/fmp4/seg0.m4s".to_string(),
                HlsFixtureResponse::binary(chunks[0].clone(), false),
            ),
            (
                "/fmp4/seg1.m4s".to_string(),
                HlsFixtureResponse::binary(chunks[1].clone(), false),
            ),
        ]);
        let base = start_hls_server(routes).await;
        let dir = tempfile::tempdir().unwrap();

        let final_task = run_task(&format!("{base}/fmp4/index.m3u8"), dir.path(), 4).await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        // fMP4 输出强制使用 .m4s 扩展名（而非源 .m3u8）。
        assert_eq!(final_task.filename, "index.m4s");
        // 初始化分片字节被前置拼接，随后才是媒体分片。
        assert_eq!(std::fs::read(final_task.output_path()).unwrap(), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hls_probes_segment_sizes_for_exact_total() {
        // 分片携带 Content-Length：HEAD 探测应取得真实字节数，
        // 最终任务的 total_size/downloaded 与拼接后的实际字节数一致。
        let chunks = [
            b"exact-size segment zero".to_vec(),
            b"exact-size segment one".to_vec(),
        ];
        let expected = chunks.concat();
        let routes = std::collections::HashMap::from([
            (
                "/sized/index.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg0.ts
#EXTINF:4.0,
seg1.ts
#EXT-X-ENDLIST
",
                ),
            ),
            (
                "/sized/seg0.ts".to_string(),
                HlsFixtureResponse::binary(chunks[0].clone(), true),
            ),
            (
                "/sized/seg1.ts".to_string(),
                HlsFixtureResponse::binary(chunks[1].clone(), true),
            ),
        ]);
        let base = start_hls_server(routes).await;
        let dir = tempfile::tempdir().unwrap();

        let final_task = run_task(&format!("{base}/sized/index.m3u8"), dir.path(), 4).await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        assert_eq!(final_task.total_size, Some(expected.len() as u64));
        assert_eq!(final_task.downloaded, expected.len() as u64);
        assert_eq!(std::fs::read(final_task.output_path()).unwrap(), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn downloads_byterange_hls_from_single_resource() {
        // 多段共享同一资源文件、靠 #EXT-X-BYTERANGE 区分；下载须按区间请求并按序拼接。
        // init=[0,1000)，seg0=[1000,2000)，seg1=[2000,3000)，拼接应还原整文件。
        let resource = make_data(3000);
        let expected = resource.clone();
        let routes = std::collections::HashMap::from([
            (
                "/br/index.m3u8".to_string(),
                HlsFixtureResponse::text(
                    "#EXTM3U
#EXT-X-TARGETDURATION:4
#EXT-X-MAP:URI=\"media.bin\",BYTERANGE=\"1000@0\"
#EXTINF:4.0,
#EXT-X-BYTERANGE:1000@1000
media.bin
#EXTINF:4.0,
#EXT-X-BYTERANGE:1000
media.bin
#EXT-X-ENDLIST
",
                ),
            ),
            (
                "/br/media.bin".to_string(),
                HlsFixtureResponse::binary(resource.clone(), true),
            ),
        ]);
        let base = start_hls_server(routes).await;
        let dir = tempfile::tempdir().unwrap();

        let final_task = run_task(&format!("{base}/br/index.m3u8"), dir.path(), 4).await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        // 三个区间长度已知，无需 HEAD：总大小直接来自区间长度之和。
        assert_eq!(final_task.total_size, Some(expected.len() as u64));
        assert_eq!(final_task.downloaded, expected.len() as u64);
        // 按区间请求并按序拼接 → 还原整文件；若忽略 Range 会拼出多份全量副本。
        assert_eq!(std::fs::read(final_task.output_path()).unwrap(), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn downloads_multiple_segments_and_verifies_checksum() {
        let data = Arc::new(make_data(2 * 1024 * 1024));
        let sha = sha256_hex(&data);
        let base = start_server(Arc::clone(&data)).await;
        let dir = tempfile::tempdir().unwrap();
        let (database, limiter, providers) = setup(dir.path());

        let task =
            DownloadTask::create(&format!("{base}/file.bin"), dir.path(), 4, "", &sha).unwrap();
        database.save_task(&task).unwrap();
        let callback: UpdateCallback = Arc::new(|_, _| {});
        let engine = DownloadEngine::new(
            task,
            Arc::clone(&database),
            limiter,
            providers,
            no_proxy_client(),
            callback,
            4,
        );

        let final_task = engine.run().await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        assert_eq!(final_task.filename, "fixture.bin");
        let output = dir.path().join(&final_task.filename);
        assert_eq!(std::fs::read(&output).unwrap(), *data);
        assert_eq!(final_task.actual_sha256.as_deref(), Some(sha.as_str()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn falls_back_to_single_stream_without_ranges() {
        let data = Arc::new(make_data(900 * 1024));
        let base = start_server(Arc::clone(&data)).await;
        let dir = tempfile::tempdir().unwrap();
        let (database, limiter, providers) = setup(dir.path());

        let task =
            DownloadTask::create(&format!("{base}/no-range.bin"), dir.path(), 8, "", "").unwrap();
        database.save_task(&task).unwrap();
        let callback: UpdateCallback = Arc::new(|_, _| {});
        let engine = DownloadEngine::new(
            task,
            Arc::clone(&database),
            limiter,
            providers,
            no_proxy_client(),
            callback,
            4,
        );

        let final_task = engine.run().await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        assert!(!final_task.supports_ranges);
        let output = dir.path().join(&final_task.filename);
        assert_eq!(std::fs::read(&output).unwrap(), *data);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_checksum_mismatch() {
        let data = Arc::new(make_data(1024 * 1024));
        let base = start_server(Arc::clone(&data)).await;
        let dir = tempfile::tempdir().unwrap();
        let (database, limiter, providers) = setup(dir.path());

        let wrong_sha = "0".repeat(64);
        let task = DownloadTask::create(&format!("{base}/file.bin"), dir.path(), 2, "", &wrong_sha)
            .unwrap();
        database.save_task(&task).unwrap();
        let callback: UpdateCallback = Arc::new(|_, _| {});
        let engine = DownloadEngine::new(
            task,
            Arc::clone(&database),
            limiter,
            providers,
            no_proxy_client(),
            callback,
            1,
        );

        let final_task = engine.run().await;

        assert_eq!(final_task.status, TaskStatus::Failed);
        assert!(final_task
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("SHA-256"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_misaligned_content_range() {
        let data = Arc::new(make_data(1024 * 1024));
        let base = start_server(Arc::clone(&data)).await;
        let dir = tempfile::tempdir().unwrap();
        let (database, limiter, providers) = setup(dir.path());

        let task = DownloadTask::create(&format!("{base}/wrong-range.bin"), dir.path(), 2, "", "")
            .unwrap();
        database.save_task(&task).unwrap();
        let engine = DownloadEngine::new(
            task,
            Arc::clone(&database),
            limiter,
            providers,
            no_proxy_client(),
            Arc::new(|_, _| {}),
            0,
        );

        let final_task = engine.run().await;

        assert_eq!(final_task.status, TaskStatus::Failed);
        assert!(final_task
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("Content-Range"));
        assert!(!final_task.output_path().exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_other_inconsistent_ranged_responses() {
        let data = Arc::new(make_data(1024 * 1024));
        let base = start_server(Arc::clone(&data)).await;

        for path in [
            "wrong-total.bin",
            "wrong-length.bin",
            "short-body.bin",
            "missing-range.bin",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (database, limiter, providers) = setup(dir.path());
            let task =
                DownloadTask::create(&format!("{base}/{path}"), dir.path(), 2, "", "").unwrap();
            database.save_task(&task).unwrap();
            let engine = DownloadEngine::new(
                task,
                Arc::clone(&database),
                limiter,
                providers,
                no_proxy_client(),
                Arc::new(|_, _| {}),
                0,
            );

            let final_task = engine.run().await;

            assert_eq!(
                final_task.status,
                TaskStatus::Failed,
                "{path} unexpectedly succeeded"
            );
            assert!(!final_task.output_path().exists());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_headers_are_used_for_probe_and_segments() {
        struct AuthProvider;

        impl DownloadProvider for AuthProvider {
            fn name(&self) -> &str {
                "auth-test"
            }

            fn can_handle(&self, _url: &str) -> bool {
                true
            }

            fn prepare(&self, task: &DownloadTask) -> Result<PreparedDownload, HttpError> {
                Ok(PreparedDownload {
                    url: task.url.clone(),
                    headers: vec![("Authorization".to_string(), "Bearer test-token".to_string())],
                })
            }
        }

        let data = Arc::new(make_data(1024 * 1024));
        let base = start_server(Arc::clone(&data)).await;
        let dir = tempfile::tempdir().unwrap();
        let database =
            Arc::new(DownloadDatabase::open(Some(dir.path().join("downloads.db"))).unwrap());
        let providers = Arc::new(ProviderRegistry::new(vec![Box::new(AuthProvider)]));
        let task =
            DownloadTask::create(&format!("{base}/private.bin"), dir.path(), 2, "", "").unwrap();
        database.save_task(&task).unwrap();
        let engine = DownloadEngine::new(
            task,
            Arc::clone(&database),
            Arc::new(RateLimiter::new(0)),
            providers,
            no_proxy_client(),
            Arc::new(|_, _| {}),
            0,
        );

        let final_task = engine.run().await;

        assert_eq!(
            final_task.status,
            TaskStatus::Completed,
            "error: {:?}",
            final_task.error
        );
        assert_eq!(std::fs::read(final_task.output_path()).unwrap(), *data);
    }
}
