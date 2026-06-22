//! 基于 `PRAGMA user_version` 的模式迁移。每个迁移步骤都会提升
//! 已存储的版本号；`apply_migrations` 仅会运行比数据库当前版本
//! 更新的步骤，从而原地升级已有的数据库。

use rusqlite::Connection;

use crate::error::StoreError;

pub const LATEST_SCHEMA_VERSION: i64 = 2;

fn create_initial_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS tasks (
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

        CREATE TABLE IF NOT EXISTS segments (
            task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            segment_index INTEGER NOT NULL,
            start_byte INTEGER NOT NULL,
            end_byte INTEGER,
            downloaded INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (task_id, segment_index)
        );
        "#,
    )
}

fn add_checksum_columns(conn: &Connection) -> rusqlite::Result<()> {
    let mut has_expected = false;
    let mut has_actual = false;
    {
        let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
        let names = stmt.query_map([], |row| row.get::<_, String>("name"))?;
        for name in names {
            match name?.as_str() {
                "expected_sha256" => has_expected = true,
                "actual_sha256" => has_actual = true,
                _ => {}
            }
        }
    }
    if !has_expected {
        conn.execute("ALTER TABLE tasks ADD COLUMN expected_sha256 TEXT", [])?;
    }
    if !has_actual {
        conn.execute("ALTER TABLE tasks ADD COLUMN actual_sha256 TEXT", [])?;
    }
    Ok(())
}

const MIGRATIONS: &[fn(&Connection) -> rusqlite::Result<()>] =
    &[create_initial_schema, add_checksum_columns];

/// 将 `conn` 升级至 [`LATEST_SCHEMA_VERSION`]，仅应用比其当前
/// `user_version` 更新的步骤。对于由未来更高 schema 版本写入的
/// 数据库会拒绝处理。
pub fn apply_migrations(conn: &Connection) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > LATEST_SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found: current,
            latest: LATEST_SCHEMA_VERSION,
        });
    }
    for (offset, migration) in MIGRATIONS.iter().enumerate() {
        let version = offset as i64 + 1;
        if version <= current {
            continue;
        }
        migration(conn)?;
        conn.pragma_update(None, "user_version", version)?;
    }
    Ok(())
}
