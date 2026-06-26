//! 任务与分段的领域模型。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CoreError;
use crate::validation::normalize_sha256;

/// 下载任务的生命周期状态。序列化为小写字符串，便于直接读取历史数据库。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Probing,
    Downloading,
    Verifying,
    Paused,
    Completed,
    Failed,
    Canceled,
}

impl TaskStatus {
    /// 占用活动工作线程槽位时返回 true。
    pub fn is_active(self) -> bool {
        matches!(
            self,
            TaskStatus::Probing | TaskStatus::Downloading | TaskStatus::Verifying
        )
    }

    /// 对应在数据库中持久化存储的小写字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Probing => "probing",
            TaskStatus::Downloading => "downloading",
            TaskStatus::Verifying => "verifying",
            TaskStatus::Paused => "paused",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Canceled => "canceled",
        }
    }

    /// 解析持久化的字符串状态；无法识别时返回 `None`。
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => TaskStatus::Queued,
            "probing" => TaskStatus::Probing,
            "downloading" => TaskStatus::Downloading,
            "verifying" => TaskStatus::Verifying,
            "paused" => TaskStatus::Paused,
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            "canceled" => TaskStatus::Canceled,
            _ => return None,
        })
    }
}

/// 下载中一段连续的字节区间。`end` 为包含端点；为 `None` 表示
/// 长度未知（无法续传的单连接下载）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub task_id: String,
    pub index: u32,
    pub start: u64,
    pub end: Option<u64>,
    #[serde(default)]
    pub downloaded: u64,
}

impl Segment {
    pub fn new(task_id: impl Into<String>, index: u32, start: u64, end: Option<u64>) -> Self {
        Self {
            task_id: task_id.into(),
            index,
            start,
            end,
            downloaded: 0,
        }
    }

    /// 下一字节请求的绝对偏移量。
    pub fn next_byte(&self) -> u64 {
        self.start + self.downloaded
    }

    /// 该区间的总长度（已知时），否则为 `None`。
    pub fn size(&self) -> Option<u64> {
        self.end.map(|end| end - self.start + 1)
    }

    /// 当已知长度的区间内每个字节都已写入时返回 true。
    pub fn complete(&self) -> bool {
        self.size().is_some_and(|size| self.downloaded >= size)
    }
}

/// 一个下载任务及其续传 / 校验所需的全部信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadTask {
    pub id: String,
    pub url: String,
    pub destination: String,
    #[serde(default)]
    pub filename: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub total_size: Option<u64>,
    #[serde(default)]
    pub downloaded: u64,
    pub connections: u32,
    #[serde(default)]
    pub supports_ranges: bool,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub expected_sha256: Option<String>,
    #[serde(default)]
    pub actual_sha256: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
}

impl DownloadTask {
    /// 构造一个新的排队任务：校验 URL 与可选的校验和，
    /// 并将并发连接数限制在 1..=32 之间。
    pub fn create(
        url: &str,
        destination: impl AsRef<Path>,
        connections: u32,
        filename: &str,
        expected_sha256: &str,
    ) -> Result<Self, CoreError> {
        let normalized_url = url.trim();
        if normalized_url.is_empty() {
            return Err(CoreError::EmptyUrl);
        }
        let checksum = normalize_sha256(expected_sha256)?;
        let now = unix_time();
        Ok(Self {
            id: Uuid::new_v4().simple().to_string(),
            url: normalized_url.to_string(),
            destination: destination.as_ref().to_string_lossy().into_owned(),
            filename: filename.to_string(),
            status: TaskStatus::Queued,
            total_size: None,
            downloaded: 0,
            connections: connections.clamp(1, 32),
            supports_ranges: false,
            etag: None,
            last_modified: None,
            expected_sha256: checksum,
            actual_sha256: None,
            error: None,
            created_at: now,
            updated_at: now,
        })
    }

    /// 下载完成后文件的最终路径。
    pub fn output_path(&self) -> PathBuf {
        Path::new(&self.destination).join(&self.filename)
    }

    /// 正在下载的 `.part` 临时文件路径。
    pub fn part_path(&self) -> PathBuf {
        let output = self.output_path();
        let name = output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        output.with_file_name(format!("{name}.part"))
    }

    /// 完成比例，范围 0.0..=1.0；当文件大小未知时为 0.0。
    pub fn progress(&self) -> f64 {
        match self.total_size {
            Some(total) if total > 0 => (self.downloaded as f64 / total as f64).min(1.0),
            _ => 0.0,
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

    #[test]
    fn segment_geometry() {
        let mut seg = Segment::new("t", 0, 100, Some(199));
        assert_eq!(seg.size(), Some(100));
        assert_eq!(seg.next_byte(), 100);
        assert!(!seg.complete());
        seg.downloaded = 100;
        assert_eq!(seg.next_byte(), 200);
        assert!(seg.complete());

        let unknown = Segment::new("t", 0, 0, None);
        assert_eq!(unknown.size(), None);
        assert!(!unknown.complete());
    }

    #[test]
    fn create_validates_and_clamps() {
        let task = DownloadTask::create("  https://x/y.zip  ", "C:/dl", 99, "", "").unwrap();
        assert_eq!(task.url, "https://x/y.zip");
        assert_eq!(task.connections, 32);
        assert_eq!(task.status, TaskStatus::Queued);
        assert_eq!(task.id.len(), 32);

        assert_eq!(
            DownloadTask::create("   ", "C:/dl", 8, "", "").unwrap_err(),
            CoreError::EmptyUrl
        );
        assert_eq!(
            DownloadTask::create("https://x", "C:/dl", 8, "", "bad").unwrap_err(),
            CoreError::InvalidSha256
        );
    }

    #[test]
    fn derives_paths_and_progress() {
        let mut task = DownloadTask::create("https://x/y", "C:/dl", 4, "file.bin", "").unwrap();
        assert_eq!(task.output_path(), Path::new("C:/dl").join("file.bin"));
        assert_eq!(task.part_path(), Path::new("C:/dl").join("file.bin.part"));
        assert_eq!(task.progress(), 0.0);
        task.total_size = Some(200);
        task.downloaded = 50;
        assert_eq!(task.progress(), 0.25);
        task.downloaded = 500;
        assert_eq!(task.progress(), 1.0);
    }

    #[test]
    fn status_round_trips_as_lowercase() {
        let json = serde_json::to_string(&TaskStatus::Downloading).unwrap();
        assert_eq!(json, "\"downloading\"");
        let parsed: TaskStatus = serde_json::from_str("\"canceled\"").unwrap();
        assert_eq!(parsed, TaskStatus::Canceled);
    }
}
