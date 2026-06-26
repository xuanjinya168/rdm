//! 下载任务数据库。

use std::path::PathBuf;
use std::sync::Mutex;

use rdm_domain::config::app_data_dir;
use rdm_domain::{DownloadTask, Segment, TaskStatus};
use rusqlite::{params, Connection};

use crate::error::StoreError;
use crate::migrations::apply_migrations;

/// 持久化存储任务及其分段的 SQLite 数据库。
///
/// 由互斥锁保护的单一连接；当内部为 `None` 时表示数据库已关闭，
/// 此后任何操作都会返回 [`StoreError::Closed`]。
pub struct DownloadDatabase {
    pub path: PathBuf,
    connection: Mutex<Option<Connection>>,
}

impl DownloadDatabase {
    /// 打开 `path` 处的数据库（必要时自动创建）；
    /// 若 `path` 为 `None`，则使用应用数据目录下的默认 `downloads.db`。
    pub fn open(path: Option<PathBuf>) -> Result<Self, StoreError> {
        let path = path.unwrap_or_else(|| app_data_dir().join("downloads.db"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&path)?;
        connection.busy_timeout(std::time::Duration::from_secs(15))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )?;
        apply_migrations(&connection)?;
        Ok(Self {
            path,
            connection: Mutex::new(Some(connection)),
        })
    }

    /// 在活动连接上运行 `f`；若数据库已关闭则直接返回错误。
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let guard = self.connection.lock().expect("database mutex poisoned");
        match guard.as_ref() {
            Some(conn) => f(conn),
            None => Err(StoreError::Closed),
        }
    }

    /// 关闭数据库连接；多次调用是幂等的。
    pub fn close(&self) {
        let mut guard = self.connection.lock().expect("database mutex poisoned");
        guard.take();
    }

    /// 插入或更新一个任务。若主键冲突，会刷新除 `created_at` 之外的
    /// 全部可变字段，`created_at` 在首次插入时确定后保持不变。
    pub fn save_task(&self, task: &DownloadTask) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO tasks (
                    id, url, destination, filename, status, total_size,
                    downloaded, connections, supports_ranges, etag,
                    last_modified, expected_sha256, actual_sha256,
                    error, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(id) DO UPDATE SET
                    url=excluded.url,
                    destination=excluded.destination,
                    filename=excluded.filename,
                    status=excluded.status,
                    total_size=excluded.total_size,
                    downloaded=excluded.downloaded,
                    connections=excluded.connections,
                    supports_ranges=excluded.supports_ranges,
                    etag=excluded.etag,
                    last_modified=excluded.last_modified,
                    expected_sha256=excluded.expected_sha256,
                    actual_sha256=excluded.actual_sha256,
                    error=excluded.error,
                    updated_at=excluded.updated_at",
                params![
                    task.id,
                    task.url,
                    task.destination,
                    task.filename,
                    task.status.as_str(),
                    task.total_size.map(|v| v as i64),
                    task.downloaded as i64,
                    task.connections,
                    task.supports_ranges as i64,
                    task.etag,
                    task.last_modified,
                    task.expected_sha256,
                    task.actual_sha256,
                    task.error,
                    task.created_at,
                    task.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    /// 加载全部任务，按创建时间倒序排列。崩溃时处于「进行中」
    /// 状态（probing/downloading/verifying）的任务会恢复为 paused；
    /// 无法识别的状态会变为 failed 并附带描述性错误。
    pub fn load_tasks(&self) -> Result<Vec<DownloadTask>, StoreError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, url, destination, filename, status, total_size,
                        downloaded, connections, supports_ranges, etag,
                        last_modified, expected_sha256, actual_sha256,
                        error, created_at, updated_at
                 FROM tasks ORDER BY created_at DESC",
            )?;
            let tasks = stmt
                .query_map([], |row| {
                    let status_str: String = row.get("status")?;
                    let raw_error: Option<String> = row.get("error")?;
                    let (status, error) = match TaskStatus::from_db_str(&status_str) {
                        Some(status) if status.is_active() => (TaskStatus::Paused, raw_error),
                        Some(status) => (status, raw_error),
                        None => (
                            TaskStatus::Failed,
                            Some(format!("Invalid persisted task status: {status_str}")),
                        ),
                    };
                    let total_size: Option<i64> = row.get("total_size")?;
                    let downloaded: i64 = row.get("downloaded")?;
                    let connections: i64 = row.get("connections")?;
                    let supports_ranges: i64 = row.get("supports_ranges")?;
                    Ok(DownloadTask {
                        id: row.get("id")?,
                        url: row.get("url")?,
                        destination: row.get("destination")?,
                        filename: row.get("filename")?,
                        status,
                        total_size: total_size.map(|v| v as u64),
                        downloaded: downloaded as u64,
                        connections: connections as u32,
                        supports_ranges: supports_ranges != 0,
                        etag: row.get("etag")?,
                        last_modified: row.get("last_modified")?,
                        expected_sha256: row.get("expected_sha256")?,
                        actual_sha256: row.get("actual_sha256")?,
                        error,
                        created_at: row.get("created_at")?,
                        updated_at: row.get("updated_at")?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(tasks)
        })
    }

    /// 删除一个任务；其分段会通过外键级联删除。
    pub fn delete_task(&self, task_id: &str) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM tasks WHERE id = ?1", [task_id])?;
            Ok(())
        })
    }

    /// 在单个事务中替换任务的全部分段。
    pub fn save_segments(&self, task_id: &str, segments: &[Segment]) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute("DELETE FROM segments WHERE task_id = ?1", [task_id])?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO segments (
                        task_id, segment_index, start_byte, end_byte, downloaded
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for segment in segments {
                    stmt.execute(params![
                        task_id,
                        segment.index,
                        segment.start as i64,
                        segment.end.map(|v| v as i64),
                        segment.downloaded as i64,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// 持久化分段的下载进度。不会修改 `end_byte`：
    /// 区间只会被 [`Self::save_segments`] 或 [`Self::split_segment`]
    /// 整体重写，因此存储的区间始终是文件的一个划分。
    pub fn update_segment(&self, segment: &Segment) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE segments SET downloaded = ?1
                 WHERE task_id = ?2 AND segment_index = ?3",
                params![segment.downloaded as i64, segment.task_id, segment.index],
            )?;
            Ok(())
        })
    }

    /// 以原子方式持久化一次动态切分：被缩短的分段的新末端
    /// 与新建的分段必须落在同一事务中；否则中途崩溃会留下
    /// 重叠或重复计数的区间，影响续传的正确性。
    pub fn split_segment(&self, shrunk: &Segment, created: &Segment) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "UPDATE segments SET end_byte = ?1, downloaded = ?2
                 WHERE task_id = ?3 AND segment_index = ?4",
                params![
                    shrunk.end.map(|v| v as i64),
                    shrunk.downloaded as i64,
                    shrunk.task_id,
                    shrunk.index,
                ],
            )?;
            tx.execute(
                "INSERT INTO segments (
                    task_id, segment_index, start_byte, end_byte, downloaded
                ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    created.task_id,
                    created.index,
                    created.start as i64,
                    created.end.map(|v| v as i64),
                    created.downloaded as i64,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// 加载任务的全部分段，按索引升序返回。
    pub fn load_segments(&self, task_id: &str) -> Result<Vec<Segment>, StoreError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT task_id, segment_index, start_byte, end_byte, downloaded
                 FROM segments WHERE task_id = ?1 ORDER BY segment_index",
            )?;
            let segments = stmt
                .query_map([task_id], |row| {
                    let index: i64 = row.get("segment_index")?;
                    let start: i64 = row.get("start_byte")?;
                    let end: Option<i64> = row.get("end_byte")?;
                    let downloaded: i64 = row.get("downloaded")?;
                    Ok(Segment {
                        task_id: row.get("task_id")?,
                        index: index as u32,
                        start: start as u64,
                        end: end.map(|v| v as u64),
                        downloaded: downloaded as u64,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(segments)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::LATEST_SCHEMA_VERSION;

    fn temp_db() -> (tempfile::TempDir, DownloadDatabase) {
        let dir = tempfile::tempdir().unwrap();
        let db = DownloadDatabase::open(Some(dir.path().join("downloads.db"))).unwrap();
        (dir, db)
    }

    fn sample_task(dir: &tempfile::TempDir) -> DownloadTask {
        DownloadTask::create("https://example.test/file", dir.path(), 8, "", "").unwrap()
    }

    #[test]
    fn round_trips_task_and_segments() {
        let (dir, db) = temp_db();
        let mut task = sample_task(&dir);
        task.status = TaskStatus::Downloading;
        task.filename = "file.bin".to_string();
        task.total_size = Some(100);
        db.save_task(&task).unwrap();
        db.save_segments(
            &task.id,
            &[
                Segment {
                    downloaded: 25,
                    ..Segment::new(&task.id, 0, 0, Some(49))
                },
                Segment {
                    downloaded: 10,
                    ..Segment::new(&task.id, 1, 50, Some(99))
                },
            ],
        )
        .unwrap();

        let loaded = &db.load_tasks().unwrap()[0];
        let segments = db.load_segments(&task.id).unwrap();

        // 崩溃前持久化的活动状态在恢复时变为暂停。
        assert_eq!(loaded.status, TaskStatus::Paused);
        assert_eq!(loaded.filename, "file.bin");
        assert_eq!(loaded.total_size, Some(100));
        assert_eq!(
            segments.iter().map(|s| s.downloaded).collect::<Vec<_>>(),
            vec![25, 10]
        );
    }

    #[test]
    fn migrates_legacy_database_adding_checksum_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        {
            let legacy = Connection::open(&path).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE tasks (
                        id TEXT PRIMARY KEY,
                        url TEXT NOT NULL,
                        destination TEXT NOT NULL,
                        filename TEXT NOT NULL DEFAULT '',
                        status TEXT NOT NULL,
                        total_size INTEGER,
                        downloaded INTEGER NOT NULL DEFAULT 0,
                        connections INTEGER NOT NULL DEFAULT 8,
                        supports_ranges INTEGER NOT NULL DEFAULT 0,
                        etag TEXT,
                        last_modified TEXT,
                        error TEXT,
                        created_at REAL NOT NULL,
                        updated_at REAL NOT NULL
                    );
                    CREATE TABLE segments (
                        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                        segment_index INTEGER NOT NULL,
                        start_byte INTEGER NOT NULL,
                        end_byte INTEGER,
                        downloaded INTEGER NOT NULL DEFAULT 0,
                        PRIMARY KEY (task_id, segment_index)
                    );",
                )
                .unwrap();
        }

        let db = DownloadDatabase::open(Some(path)).unwrap();
        let mut task = DownloadTask::create(
            "https://example.test/file",
            dir.path(),
            8,
            "",
            &"a".repeat(64),
        )
        .unwrap();
        task.actual_sha256 = Some("b".repeat(64));
        db.save_task(&task).unwrap();

        let loaded = &db.load_tasks().unwrap()[0];
        assert_eq!(
            loaded.expected_sha256.as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert_eq!(
            loaded.actual_sha256.as_deref(),
            Some("b".repeat(64).as_str())
        );
    }

    #[test]
    fn close_is_idempotent_and_blocks_further_use() {
        let (_dir, db) = temp_db();
        db.close();
        db.close();
        assert!(matches!(db.load_tasks(), Err(StoreError::Closed)));
    }

    #[test]
    fn records_current_schema_version() {
        let (_dir, db) = temp_db();
        let version = db
            .with_conn(|conn| {
                Ok(conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))?)
            })
            .unwrap();
        assert!(version >= LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn marks_unknown_status_as_failed() {
        let (dir, db) = temp_db();
        let task = sample_task(&dir);
        db.save_task(&task).unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE tasks SET status = ?1 WHERE id = ?2",
                params!["not-a-status", task.id],
            )?;
            Ok(())
        })
        .unwrap();

        let loaded = &db.load_tasks().unwrap()[0];
        assert_eq!(loaded.status, TaskStatus::Failed);
        assert_eq!(
            loaded.error.as_deref(),
            Some("Invalid persisted task status: not-a-status")
        );
    }

    #[test]
    fn split_segment_is_atomic_and_visible() {
        let (dir, db) = temp_db();
        let task = sample_task(&dir);
        db.save_task(&task).unwrap();
        db.save_segments(&task.id, &[Segment::new(&task.id, 0, 0, Some(999))])
            .unwrap();

        let mut shrunk = Segment::new(&task.id, 0, 0, Some(499));
        shrunk.downloaded = 100;
        let created = Segment::new(&task.id, 1, 500, Some(999));
        db.split_segment(&shrunk, &created).unwrap();

        let segments = db.load_segments(&task.id).unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].end, Some(499));
        assert_eq!(segments[0].downloaded, 100);
        assert_eq!(segments[1].start, 500);
        assert_eq!(segments[1].end, Some(999));
    }
}
