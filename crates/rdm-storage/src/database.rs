//! The download database. Port of the Python `DownloadDatabase`.

use std::path::PathBuf;
use std::sync::Mutex;

use rdm_domain::config::app_data_dir;
use rdm_domain::{DownloadTask, Segment, TaskStatus};
use rusqlite::{params, Connection};

use crate::error::StoreError;
use crate::migrations::apply_migrations;

/// A persistent SQLite store for tasks and their segments.
///
/// Holds one connection behind a mutex; `None` means the database has been
/// closed and every operation thereafter fails with [`StoreError::Closed`].
pub struct DownloadDatabase {
    pub path: PathBuf,
    connection: Mutex<Option<Connection>>,
}

impl DownloadDatabase {
    /// Open (creating if needed) the database at `path`, or the default
    /// `downloads.db` under the app data directory when `None`.
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

    /// Run `f` with the live connection, or fail if the database is closed.
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

    /// Close the connection. Idempotent.
    pub fn close(&self) {
        let mut guard = self.connection.lock().expect("database mutex poisoned");
        guard.take();
    }

    /// Insert or update a task. On conflict every mutable field is refreshed
    /// except `created_at`, which is fixed at first insert.
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

    /// Load every task, newest first. Statuses left mid-flight by a crash
    /// (probing/downloading/verifying) come back as paused; an unrecognized
    /// stored status becomes failed with a descriptive error.
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

    /// Delete a task; its segments cascade away via the foreign key.
    pub fn delete_task(&self, task_id: &str) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM tasks WHERE id = ?1", [task_id])?;
            Ok(())
        })
    }

    /// Replace all segments of a task in one transaction.
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

    /// Persist a segment's progress. Leaves `end_byte` untouched: ranges are
    /// only ever rewritten atomically by [`Self::save_segments`] or
    /// [`Self::split_segment`], so stored ranges always partition the file.
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

    /// Persist a dynamic split atomically: the shrunk segment's new end and the
    /// freshly created segment must land in one transaction, or a crash in
    /// between would leave overlapping or double-counted ranges for resume.
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

    /// Load a task's segments ordered by index.
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

        // An active status persisted before a crash resumes as paused.
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
