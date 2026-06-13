//! The segmented download engine. Async (tokio) port of the Python
//! `downloader.engine` module.
//!
//! The Python engine ran each download on its own thread with a worker pool and
//! interrupted blocked socket reads by closing the shared client. Here a single
//! [`DownloadEngine::run`] future drives the whole download, segment workers are
//! tokio tasks, and pause/cancel is delivered through a
//! [`CancellationToken`] that interrupts in-flight reads via `select!`.

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::header::{IF_RANGE, RANGE};
use sha2::{Digest, Sha256};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use rdm_domain::segments::{build_segments, valid_resume_segments};
use rdm_domain::validation::sanitize_filename;
use rdm_domain::{DownloadTask, Segment, TaskStatus};
use rdm_http::{probe_url, PreparedDownload, ProbeResult, ProviderRegistry};
use rdm_storage::DownloadDatabase;

use crate::error::EngineError;
use crate::files::{publish_part_file, reserve_part_file};
use crate::rate_limit::RateLimiter;
use crate::sparse::mark_sparse;

const MIN_SEGMENT_SIZE: u64 = 512 * 1024;
/// A busy segment is only split while at least `2 * MIN_SPLIT_SIZE` is left, so
/// both halves stay worth a request and a network chunk already in flight when
/// the split lands (far smaller than half this) cannot cross the new end.
const MIN_SPLIT_SIZE: u64 = 1024 * 1024;

// Speed samples are folded into 50 ms buckets over a 3 s window.
const SPEED_BUCKETS_PER_SECOND: u64 = 20;
const SPEED_WINDOW_BUCKETS: u64 = 3 * SPEED_BUCKETS_PER_SECOND;

const UI_NOTIFY_INTERVAL: Duration = Duration::from_millis(200);
const DB_SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Called on every throttled progress update with a task snapshot and the
/// current speed in bytes per second.
pub type UpdateCallback = Arc<dyn Fn(DownloadTask, f64) + Send + Sync>;

/// Pause/cancel/failure signals shared between the engine and its workers.
struct Signals {
    pause: AtomicBool,
    cancel: AtomicBool,
    failure: AtomicBool,
    /// Cancelled whenever any signal is raised, to interrupt blocked reads.
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

/// A handle for pausing or canceling a running engine from elsewhere.
#[derive(Clone)]
pub struct EngineHandle {
    signals: Arc<Signals>,
}

impl EngineHandle {
    /// Request a pause; the run finishes in the `Paused` state.
    pub fn pause(&self) {
        self.signals.request_pause();
    }

    /// Request a cancel; the run finishes in the `Canceled` state.
    pub fn cancel(&self) {
        self.signals.request_cancel();
    }
}

/// Mutable download state shared between workers, guarded by one mutex.
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

    /// Steal the far half of the busiest active segment, returning the
    /// `(victim, created)` indices. The victim keeps streaming its trimmed
    /// range so no byte is written twice. Caller holds the state lock.
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

/// Per-segment values snapshotted before a request, so no lock is held across
/// the network read.
struct SegmentRequest {
    next_byte: u64,
    end: Option<u64>,
    expects_partial: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    part_path: PathBuf,
}

/// A configured, ready-to-run download for a single task.
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
    /// Create an engine. `client` should be sized for the task's connection
    /// count (e.g. via `rdm_http::build_client`).
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

    /// A handle for pausing/canceling this engine while it runs.
    pub fn handle(&self) -> EngineHandle {
        EngineHandle {
            signals: Arc::clone(&self.signals),
        }
    }

    /// Run the download to completion, returning the final task. Errors are
    /// reported through the task's status/error fields, never as a panic.
    pub async fn run(self) -> DownloadTask {
        let prepared = match self.providers.prepare(&self.task) {
            Ok(prepared) => prepared,
            Err(error) => return self.fail_immediately(error.to_string()),
        };
        let inner = Arc::new(EngineInner {
            database: self.database,
            limiter: self.limiter,
            client: self.client,
            callback: self.callback,
            retry_count: self.retry_count,
            signals: self.signals,
            prepared_headers: prepared.headers.clone(),
            download_url: Mutex::new(prepared.url.clone()),
            state: Mutex::new(EngineState::new(self.task)),
            notify: Mutex::new(NotifyState {
                last_ui: Instant::now() - DB_SAVE_INTERVAL,
                last_db: Instant::now() - DB_SAVE_INTERVAL,
            }),
        });

        match inner.execute(&prepared).await {
            Ok(()) => {}
            Err(EngineError::Interrupted) => inner.set_status(inner.aborted_status(), None),
            Err(error) => {
                if inner.signals.should_abort() {
                    inner.set_status(inner.aborted_status(), None);
                } else {
                    inner.set_status(TaskStatus::Failed, Some(error.to_string()));
                }
            }
        }
        let final_task = inner.state().task.clone();
        final_task
    }

    fn fail_immediately(self, error: String) -> DownloadTask {
        let mut task = self.task;
        task.status = TaskStatus::Failed;
        task.error = Some(error);
        task.updated_at = unix_time();
        let _ = self.database.save_task(&task);
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
    prepared_headers: Vec<(String, String)>,
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

    async fn execute(self: &Arc<Self>, prepared: &PreparedDownload) -> Result<(), EngineError> {
        self.set_status(TaskStatus::Probing, None);
        let probe = probe_url(&self.client, &prepared.url).await?;
        *self
            .download_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = probe.final_url.clone();
        self.prepare_task(&probe)?;
        self.prepare_segments()?;
        self.check_interrupted()?;
        self.set_status(TaskStatus::Downloading, None);
        self.download_segments().await?;
        if self.signals.is_canceled() {
            self.set_status(TaskStatus::Canceled, None);
        } else if self.signals.is_paused() {
            self.set_status(TaskStatus::Paused, None);
        } else {
            self.finish().await?;
        }
        Ok(())
    }

    /// Decide resume-vs-fresh, reserve the output name, and update metadata.
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
                sanitize_filename(&filename)
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
        self.notify(true);
        Ok(())
    }

    /// Build (or restore) the segment list and prepare the `.part` file.
    fn prepare_segments(&self) -> Result<(), EngineError> {
        let resume = std::mem::take(&mut self.state().resume_segments);
        if !resume.is_empty() {
            let downloaded: u64 = resume.iter().map(|s| s.downloaded).sum();
            let mut st = self.state();
            st.task.downloaded = downloaded;
            st.segments = resume;
            drop(st);
            self.notify(true);
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
        self.notify(true);
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
            // More workers than pending segments is useful: an idle worker
            // immediately splits a busy segment, so a resumed download with one
            // large leftover still parallelizes.
            let ideal = (pending.len() as u64).max(remaining / MIN_SPLIT_SIZE);
            (st.task.connections as u64).min(ideal).max(1) as u32
        } else {
            1
        }
    }

    async fn segment_worker(&self) -> Result<(), EngineError> {
        loop {
            self.check_interrupted()?;
            let index = match self.next_segment() {
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

    fn next_segment(&self) -> Option<u32> {
        let mut st = self.state();
        while let Some(index) = st.pending.pop_front() {
            if st.segments[index as usize].complete() {
                continue;
            }
            st.active.insert(index);
            return Some(index);
        }
        let (victim_index, created_index) = st.split_largest_active()?;
        // Persisted inside the lock: two rapid splits of the same victim must
        // commit in the order they were applied, or a crash could leave
        // overlapping ranges for resume.
        let victim = st.segments[victim_index as usize].clone();
        let created = st.segments[created_index as usize].clone();
        let _ = self.database.split_segment(&victim, &created);
        Some(created_index)
    }

    async fn download_segment(&self, index: u32) -> Result<(), EngineError> {
        let mut attempts = 0u32;
        loop {
            self.check_interrupted()?;
            let result = self.download_segment_once(index).await;
            let segment = self.state().segments[index as usize].clone();
            let _ = self.database.update_segment(&segment);
            match result {
                Ok(()) => return Ok(()),
                Err(EngineError::Interrupted) => return Err(EngineError::Interrupted),
                Err(error) => {
                    // A pause/cancel makes any in-flight I/O error that
                    // interruption, not a download failure.
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
        for (name, value) in &self.prepared_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
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
                let _ = self.database.update_segment(&segment);
                checkpoint = Instant::now();
            }
            self.notify(false);
        }

        let (downloaded, size) = {
            let st = self.state();
            let segment = &st.segments[index as usize];
            (segment.downloaded, segment.size())
        };
        if let Some(size) = size {
            if downloaded != size {
                return Err(EngineError::Download(format!(
                    "Segment ended early ({downloaded}/{size} bytes)"
                )));
            }
        }
        Ok(())
    }

    /// Trim a freshly read chunk against the segment's current end, which a
    /// concurrent split may have shrunk. Returns empty when nothing remains.
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
            self.set_status(TaskStatus::Verifying, None);
            let actual_sha = self.calculate_sha256(&part_path).await?;
            self.state().task.actual_sha256 = Some(actual_sha.clone());
            if actual_sha != expected_sha {
                // The bytes are complete but wrong; drop them so a retry starts
                // fresh instead of re-hashing the same corrupt file.
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
        self.set_status(TaskStatus::Completed, None);
        Ok(())
    }

    fn discard_partial(&self, task_id: &str, part_path: &Path) -> Result<(), EngineError> {
        self.database.save_segments(task_id, &[])?;
        self.state().task.downloaded = 0;
        let _ = std::fs::remove_file(part_path);
        Ok(())
    }

    /// Hash the finished file off the async runtime: reading and digesting a
    /// multi-gigabyte file is pure blocking work, so it runs on the blocking
    /// pool instead of stalling a tokio worker thread. Safe to read without a
    /// lock because every segment worker has already joined by `finish`.
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

    fn set_status(&self, status: TaskStatus, error: Option<String>) {
        {
            let mut st = self.state();
            st.task.status = status;
            st.task.error = error;
        }
        self.notify(true);
    }

    /// Throttled progress fan-out: persists the task at most once a second and
    /// invokes the callback at most every 200 ms (or immediately when forced).
    fn notify(&self, force: bool) {
        let now = Instant::now();
        let save_to_db;
        {
            let mut notify = self
                .notify
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !force && now.duration_since(notify.last_ui) < UI_NOTIFY_INTERVAL {
                return;
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
            let _ = self.database.save_task(&snapshot);
        }
        (self.callback)(snapshot, speed);
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    fn make_data(size: usize) -> Vec<u8> {
        (0..size).map(|i| (i % 251) as u8).collect()
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        to_hex(&hasher.finalize())
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
        for line in lines {
            if let Some(value) = line
                .strip_prefix("Range:")
                .or_else(|| line.strip_prefix("range:"))
            {
                range = Some(value.trim().to_string());
            }
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
        headers.push_str(&format!("Content-Length: {}\r\n", body.len()));
        headers.push_str("ETag: \"v1\"\r\n");
        if honor_range {
            headers.push_str("Accept-Ranges: bytes\r\n");
        }
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
}
