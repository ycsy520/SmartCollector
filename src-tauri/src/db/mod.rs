//! SQLite 持久层：r2d2 连接池、WAL、foreign_keys=ON、FTS5、迁移入口。
//! 表结构与查询以 docs/arch/03-SQLite表结构.md 为唯一契约源。
pub mod config_store;
pub mod fragments;
pub mod habits;
pub mod jobs;
pub mod migrations;
pub mod results;
pub mod skills;
pub mod triage;
pub mod usage;

use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use r2d2_sqlite::SqliteConnectionManager;

use crate::error::AppError;

pub type Pool = r2d2::Pool<SqliteConnectionManager>;

/// ISO 8601 UTC，统一时间戳格式（03 文档引擎约定）。
pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// N 天前的同格式时间戳。与 `now_iso` 同格式，故 ISO 串可直接做字典序比较（周计量窗口用）。
pub fn iso_days_ago(days: i64) -> String {
    (Utc::now() - chrono::Duration::days(days)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// 打开（或创建）应用数据库并执行迁移。每个连接启用 WAL + 外键。
pub fn init_pool(db_path: &Path) -> Result<Pool, AppError> {
    // sqlite-vec 经 auto_extension 注册；须早于任何连接打开，之后新建连接自动挂上 vec0。
    crate::vector::sqlite_vec::ensure_extension_registered();
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| AppError::DbWrite(e.to_string()))?;
    }
    let manager = SqliteConnectionManager::file(db_path).with_init(|c| {
        // journal_mode 是库级持久设置，重复设置无副作用；foreign_keys 是连接级，必须每连接设置。
        // execute_batch（sqlite3_exec 语义）可吞掉 PRAGMA 的返回行，pragma_update 不行。
        c.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
    });
    let pool = r2d2::Pool::builder()
        .max_size(4)
        .connection_timeout(Duration::from_secs(5))
        .build(manager)?;
    let conn = pool.get()?;
    // 迁移是本项目唯一会**改写用户既有数据**的动作，而用户的库只有这一份，故先留一份迁移前快照。
    if migrations::needs_migration(&conn)? {
        if let Err(e) = snapshot_before_migration(&conn, db_path) {
            // 备份写不成不阻断启动：迁移已是"要么全成要么全无"，快照只是多给一代退路。
            // 因为写不出备份就不让用户读自己的数据，是更坏的结果。
            eprintln!("[smart-collector] 迁移前快照未写成: {e}");
        }
    }
    migrations::migrate(&conn)?;
    drop(conn);
    let conn = pool.get()?;
    config_store::ensure_defaults(&conn, &crate::config::AppConfig::default())?;
    skills::ensure_builtin(&conn)?;
    Ok(pool)
}

/// 把库文件复制成 `<db>.pre-migrate.bak`（单代，每次迁移前覆盖）。
/// 必须先 `wal_checkpoint(TRUNCATE)`：WAL 下最近的写入还在 `-wal` 里，只拷 `.db` 拷出来
/// 是一份落后于现实的镜像，真到还原那天反而丢数据。checkpoint 失败就干脆不备份——
/// 宁可没有备份，也不要一份骗人的备份。
fn snapshot_before_migration(conn: &rusqlite::Connection, db_path: &Path) -> Result<(), AppError> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    if version == 0 {
        // 全新空库：没有既有数据可失去，留个 .bak 只是往用户目录扔垃圾。
        return Ok(());
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| AppError::DbWrite(format!("迁移前 checkpoint 失败: {e}")))?;
    let name = db_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::DbWrite("库文件名无法编码为 UTF-8，跳过备份".to_string()))?;
    std::fs::copy(db_path, db_path.with_file_name(format!("{name}.pre-migrate.bak")))
        .map_err(|e| AppError::DbWrite(format!("迁移前备份失败: {e}")))?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrations::migrate(&conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_open_runs_migrations_and_pragmas() {
        let dir = std::env::temp_dir().join(format!("sc-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("smart.db");
        let pool = init_pool(&path).unwrap();
        let conn = pool.get().unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, migrations::LATEST_VERSION as i64);
        drop(pool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 老库升级要留一份**迁移前**的快照：备份文件里的 schema 必须还是升级前的样子，
    /// 否则拿它还原等于没还原。
    #[test]
    fn upgrading_a_v6_db_snapshots_the_pre_migration_image() {
        let dir = std::env::temp_dir().join(format!("sc-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("smart.db");
        let bak = dir.join("smart.db.pre-migrate.bak");
        {
            // 手造一个只到 v6 的库（v3 的重建表需要关外键）。
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
            for step in migrations::MIGRATIONS.iter().take(6) {
                conn.execute_batch(step.1).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 6;").unwrap();
            conn.execute(
                "INSERT INTO fragments(id,content,source,content_hash,char_count,created_at,updated_at)
                 VALUES('f1','旧库正文','manual','h',4,'2026-01-01','2026-01-01')",
                [],
            )
            .unwrap();
        }
        assert!(!bak.exists(), "还没迁移不该有快照");

        let pool = init_pool(&path).unwrap();
        let live: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM pragma_table_info('fragments') WHERE name='media_path'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 1, "v7 应已给现库加上 media_path");
        drop(pool);

        assert!(bak.exists(), "迁移前应留快照");
        let snap = rusqlite::Connection::open(&bak).unwrap();
        let in_snap: i64 = snap
            .query_row(
                "SELECT count(*) FROM pragma_table_info('fragments') WHERE name='media_path'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_snap, 0, "快照必须是迁移**前**的镜像，不是迁移后的副本");
        let content: String = snap
            .query_row("SELECT content FROM fragments WHERE id='f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(content, "旧库正文");
        drop(snap);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
