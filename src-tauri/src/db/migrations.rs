//! PRAGMA user_version 递增迁移，只加列不改列（03 文档 §6）。
//! 规则：MIGRATIONS 顺序执行、每步要么全成要么全无；禁止修改已发布版本的 SQL。

use rusqlite::Connection;

use crate::error::AppError;

/// (目标 user_version, SQL, 是否自带事务)。
/// 第三项为 `true` 表示这段 SQL 已经用 `BEGIN;…COMMIT;` 自己括好了，迁移器**不得**再包一层——
/// SQLite 不允许事务套事务，里面那句 `BEGIN` 会直接报
/// "cannot start a transaction within a transaction"。已发布的文件不许改，所以只能迁就它。
/// 目前只有 `schema_v2.sql` 是这种情况（全库零触发器，`BEGIN;` 只可能出现在这里）。
pub const MIGRATIONS: &[(u32, &str, bool)] = &[
    (1, include_str!("schema.sql"), false),
    (2, include_str!("schema_v2.sql"), true), // UI v2 扩展（03 §3.9：分拣层/附言列 + skills/habits 表）
    (3, include_str!("schema_v3.sql"), false), // 批次2：状态机加入 'skipped'（重建 fragment_status CHECK）
    (4, include_str!("schema_v4.sql"), false), // 批次6：原文可编辑（content_updated_at + edit_log 两列）
    (5, include_str!("schema_v5.sql"), false), // 批次9：数据迁移——一次性打开内置技能开关
    (6, include_str!("schema_v6.sql"), false), // 批次17：llm_calls 表——大模型调用计量（只元数据，无正文）
    (7, include_str!("schema_v7.sql"), false), // 批次21-B：fragments.media_path——图片原样收藏的文件名
    (8, include_str!("schema_v8.sql"), false), // P0-1：fragment_status.outbound_ok——隐私出口授权落库
];

/// 与 MIGRATIONS 末元素一致（新增迁移时同步 +1）。
pub const LATEST_VERSION: u32 = 8;

/// 迁移终点的版本 = MIGRATIONS 末元素的 target。真正的判据是它，不是 `LATEST_VERSION`
/// 那个手写常量（常量会忘记同步，列表不会——加一步就自动生效）。
fn expected_version() -> u32 {
    MIGRATIONS.last().map(|(v, _, _)| *v).unwrap_or(0)
}

/// 是否存在需要执行的迁移（调用方据此决定是否先备份）。
pub fn needs_migration(conn: &Connection) -> Result<bool, AppError> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    Ok(version < expected_version())
}

pub fn migrate(conn: &Connection) -> Result<(), AppError> {
    migrate_through(conn, MIGRATIONS)
}

/// 逐步前滚，**每一步一个事务**：SQL 与它的 `user_version` 写回同生同死。
///
/// 为什么必须这样：旧实现把 `execute_batch(sql)` 和 `PRAGMA user_version` 分成两次独立提交，
/// 中间被杀（进程崩、断电、Ctrl+C）就留下"表结构已改、版本号没改"的库；重跑时 v7 的
/// `ALTER TABLE ADD COLUMN`（SQLite 无 IF NOT EXISTS）第二次直接 duplicate column 报错，
/// 于是每次启动都失败 —— 用户唯一的一份数据被永久锁死，且 `panic="abort"` + `strip` 下
/// 屏幕上连一行字都没有。事务化后失败即回滚，版本号留在原地，下次启动从头重放这一步。
fn migrate_through(conn: &Connection, steps: &[(u32, &str, bool)]) -> Result<(), AppError> {
    let mut version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let target_last = steps.last().map(|(v, _, _)| *v).unwrap_or(version);
    if version >= target_last {
        return Ok(());
    }

    // foreign_keys 是**连接级**且**在事务内是 no-op**（SQLite 直接忽略），而 v3 要
    // DROP + RENAME fragment_status，开着 FK 做不了。所以只能在 BEGIN 之前关、提交之后恢复。
    // synchronous 拉到 FULL(2)：WAL 下 NORMAL(1) 只在 checkpoint 落盘，迁移这几笔提交是用户
    // 数据的唯一副本，必须扛得住断电而不只是进程崩溃。
    // 恢复用**数字**而不是关键字：PRAGMA synchronous 的整数档是 0=Off 1=Normal 2=Full 3=Extra，
    // 关键字写法会把连接改成固定一档，而连接原本可能是别值（内存库默认就是 2）。
    // 迁移该只做临时加压、结束后原样交还，不该顺手改写调用方的设置。
    let sync_before = int_pragma(conn, "synchronous")?;
    set_pragma(conn, "foreign_keys", "OFF")?;
    set_pragma(conn, "synchronous", "FULL")?;

    let rolled = run_steps(conn, &mut version, steps);
    // 无论成败都恢复连接级设置：失败路径上调用方还会继续用这条连接
    // （init_pool 的 ensure_defaults / ensure_builtin），留着 OFF 等于把防护留在表外。
    let restored = set_pragma(conn, "synchronous", &sync_before.to_string())
        .and_then(|_| set_pragma(conn, "foreign_keys", "ON"));
    rolled?;
    restored?;

    let got: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    if got != target_last {
        return Err(AppError::DbWrite(format!(
            "迁移后 user_version={got}，期望 {target_last}"
        )));
    }
    Ok(())
}

fn run_steps(
    conn: &Connection,
    version: &mut u32,
    steps: &[(u32, &str, bool)],
) -> Result<(), AppError> {
    for (target, sql, self_tx) in steps {
        if *version >= *target {
            continue;
        }
        // BEGIN IMMEDIATE：一开始就拿写锁。deferred 事务会在真正写入时才升级，
        // 那时撞上别的连接（worker 线程）就是 SQLITE_BUSY，而忙等已经花了。
        // 自带事务的那步（v2）不包：它的 DDL 原子性由文件里的 BEGIN…COMMIT 给，
        // 残留的只有"COMMIT 之后、版本号写回之前"这一小截窗口 —— 想彻底关掉就得改
        // 已发布文件，二者取其次。
        if !*self_tx {
            conn.execute_batch("BEGIN IMMEDIATE;")
                .map_err(|e| AppError::DbWrite(format!("开启 v{target} 迁移事务失败: {e}")))?;
        }
        let applied = apply_step(conn, *target, sql);
        match applied {
            Ok(()) => {
                if !*self_tx {
                    if let Err(e) = conn.execute_batch("COMMIT;") {
                        let _ = conn.execute_batch("ROLLBACK;");
                        return Err(AppError::DbWrite(format!("提交 v{target} 迁移失败: {e}")));
                    }
                }
            }
            Err(e) => {
                if !*self_tx {
                    let _ = conn.execute_batch("ROLLBACK;");
                }
                return Err(e);
            }
        }
        *version = *target;
    }
    Ok(())
}

fn apply_step(conn: &Connection, target: u32, sql: &str) -> Result<(), AppError> {
    conn.execute_batch(sql)
        .map_err(|e| AppError::DbWrite(format!("迁移到 v{target} 失败: {e}")))?;
    conn.execute_batch(&format!("PRAGMA user_version = {target};"))
        .map_err(|e| AppError::DbWrite(format!("写回 user_version={target} 失败: {e}")))
}

/// PRAGMA 不能用占位符参数化，只能拼接；取值全是本文件内的常量，无外部输入。
fn set_pragma(conn: &Connection, name: &str, value: &str) -> Result<(), AppError> {
    conn.execute_batch(&format!("PRAGMA {name} = {value};"))
        .map_err(|e| AppError::DbWrite(format!("设置 PRAGMA {name}={value} 失败: {e}")))
}

/// 读整型 PRAGMA（`synchronous` / `user_version` 这类）。名字来自本文件常量，无外部输入。
fn int_pragma(conn: &Connection, name: &str) -> Result<i64, AppError> {
    conn.query_row(&format!("PRAGMA {name};"), [], |r| r.get(0))
        .map_err(|e| AppError::DbRead(format!("读取 PRAGMA {name} 失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_migrates_to_latest() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, LATEST_VERSION);
        // 核心表全部就位
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type IN ('table','view')
                   AND name IN ('fragments','fragment_status','processing_results',
                                'vector_refs','config','deletions','fts_fragments')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 7);
    }

    #[test]
    fn v2_adds_review_columns_and_skill_habit_tables() {
        let conn = crate::db::test_conn();
        // skills / habits 两张新表就位
        let t: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'
                   AND name IN ('skills','habits')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(t, 2);
        // fragments 新列存在且默认值正确
        let row: (String, String, i64, Option<String>) = conn
            .query_row(
                "INSERT INTO fragments(id,content,title,source,content_hash,char_count,created_at,updated_at)
                 VALUES('f1','x',NULL,'manual','h1',1,'2026-01-01','2026-01-01')
                 RETURNING layer, media_type, reviewed, archived_by",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "buffer");
        assert_eq!(row.1, "text");
        assert_eq!(row.2, 0);
        assert_eq!(row.3, None);
    }

    #[test]
    fn v2_dedup_index_excludes_trash() {
        let conn = crate::db::test_conn();
        let ins = |id: &str, hash: &str, layer: &str, conn: &Connection| {
            conn.execute(
                "INSERT INTO fragments(id,content,source,content_hash,char_count,created_at,updated_at,layer)
                 VALUES(?1,'x','manual',?2,1,'2026-01-01','2026-01-01',?3)",
                rusqlite::params![id, hash, layer],
            )
            .is_ok()
        };
        assert!(ins("a", "dup", "buffer", &conn));
        // 非 trash 层重复 hash 被唯一索引拦下
        assert!(!ins("b", "dup", "archived", &conn));
        // trash 层不受去重约束（允许重收集）
        assert!(ins("c", "dup", "trash", &conn));
    }

    #[test]
    fn v3_admits_skipped_and_preserves_rows() {
        // 造一个仅到 v2 的旧库，塞一行 done 记录，再前滚到 v3：数据应存活、CHECK 应放行 skipped。
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        conn.execute_batch(include_str!("schema_v2.sql")).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute(
            "INSERT INTO fragments(id,content,source,content_hash,char_count,created_at,updated_at)
             VALUES('f1','旧库正文','manual','h',4,'2026-01-01','2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO fragment_status(fragment_id,status,enqueued_at) VALUES('f1','done','2026-01-01')",
            [],
        )
        .unwrap();
        // 旧 CHECK 拒绝 skipped
        assert!(conn
            .execute("UPDATE fragment_status SET status='skipped' WHERE fragment_id='f1'", [])
            .is_err());
        migrate(&mut conn).unwrap(); // 前滚 v2→v3
        let preserved: String = conn
            .query_row("SELECT status FROM fragment_status WHERE fragment_id='f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(preserved, "done"); // 迁移未丢数据
        conn.execute("UPDATE fragment_status SET status='skipped' WHERE fragment_id='f1'", [])
            .unwrap(); // 新 CHECK 放行 skipped
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, LATEST_VERSION);
    }

    /// 批次9-P0c：老库的内置技能一直是 `enabled=0`（用户从未见过"换个方向再加工"入口），
    /// 迁移 5 一次性打开；此后再迁移不得覆盖用户手动关闭的选择。
    #[test]
    fn v5_enables_builtin_skills_once_and_never_again() {
        // 手造一个只到 v4 的库：`test_conn()` 已是 v7，把 user_version 拨回 4 会让 v7 的
        // ALTER ADD COLUMN 重放第二次（SQLite 无 IF NOT EXISTS，直接 duplicate column 报错）。
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        for step in MIGRATIONS.iter().take(4) {
            conn.execute_batch(step.1).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();
        crate::db::skills::ensure_builtin(&conn).unwrap();
        conn.execute("UPDATE skills SET enabled = 0 WHERE builtin = 1", [])
            .unwrap();
        migrate(&mut conn).unwrap();
        let on: i64 = conn
            .query_row(
                "SELECT count(*) FROM skills WHERE builtin = 1 AND enabled = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            on,
            crate::db::skills::BUILTINS.len() as i64,
            "全部内置技能应被一次性打开"
        );
        // 用户随后自己关掉：重复迁移必须尊重它
        conn.execute("UPDATE skills SET enabled = 0 WHERE id = 'builtin:verify'", [])
            .unwrap();
        migrate(&conn).unwrap();
        let still: i64 = conn
            .query_row("SELECT enabled FROM skills WHERE id = 'builtin:verify'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still, 0);
    }

    /// 批次21-B：图片文件名一列。老库（v6）前滚须保住既有行，且新列默认 NULL
    /// ——非图片片段没有文件，NULL 就是"没有"，不用空串冒充。
    #[test]
    fn v7_adds_media_path_and_preserves_rows() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        for step in MIGRATIONS.iter().take(6) {
            conn.execute_batch(step.1).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 6;").unwrap();
        conn.execute(
            "INSERT INTO fragments(id,content,source,content_hash,char_count,created_at,updated_at)
             VALUES('f1','老库正文','manual','h',4,'2026-01-01','2026-01-01')",
            [],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let preserved: String = conn
            .query_row("SELECT content FROM fragments WHERE id='f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(preserved, "老库正文");
        let path: Option<String> = conn
            .query_row("SELECT media_path FROM fragments WHERE id='f1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(path, None);
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, LATEST_VERSION);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = crate::db::test_conn();
        migrate(&conn).unwrap(); // 二次执行不应报错（IF NOT EXISTS 幂等）
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, LATEST_VERSION);
    }

    #[test]
    fn older_db_upgrades_step_by_step() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        // 手工建 0 号库：无表。迁移后应到最新。
        migrate(&mut conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, LATEST_VERSION);
    }

    fn user_version(conn: &Connection) -> u32 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
    }

    /// 一步迁移里"前半成功、后半报错"必须整笔回滚：不留半成品表/列/行，版本号停在原地，
    /// 下一次启动能把这一步**从头重放**（旧实现做不到，于是永久卡在启动）。
    #[test]
    fn failed_step_rolls_back_completely_and_replays_clean() {
        const GOOD: &str = "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT);
                            INSERT INTO t(v) VALUES('keep');";
        // 同一步内先做真改动，再用 NOT NULL 违约把批处理打断在中间。
        const BROKEN: &str = "ALTER TABLE t ADD COLUMN extra TEXT;
                              INSERT INTO t(v) VALUES('半成品');
                              CREATE TABLE broken(x TEXT NOT NULL);
                              INSERT INTO broken(x) VALUES(NULL);";
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // 基线取当前值而不是写死数字：新建内存连接的 synchronous 默认就是 Full(2)，
        // 而 init_pool 的文件连接是 Normal(1)。要断言的是"迁移把连接原样交还"，
        // 不是某个具体档位——写死任何一个都会在另一类连接上假通过或假失败。
        let sync_before: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        let steps: [(u32, &str, bool); 2] = [(1, GOOD, false), (2, BROKEN, false)];

        let err = migrate_through(&conn, &steps).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("v2"), "错误应点明是哪一步失败: {msg}");

        assert_eq!(user_version(&conn), 1, "失败的那一步不得写回版本号");
        // 用 prepare 探测列是否存在：`execute("SELECT …")` 对任何 SELECT 都会因为
        // "Execute returned results" 报错，is_err() 于是恒真，等于什么都没断言。
        assert!(
            conn.prepare("SELECT extra FROM t").is_err(),
            "回滚后新增列不应存在"
        );
        let orphan: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='broken'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan, 0, "回滚后半成品表不应留下痕迹");
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "回滚后只应有上一步的行");

        // 连接级设置必须被恢复：调用方接下来还要用这条连接写业务数据。
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "迁移失败也得把 foreign_keys 恢复成 ON");
        let sync: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, sync_before, "恢复到迁移前的值");

        // 同一步修好后重放：已完成的第 1 步跳过，第 2 步干净落地。
        let fixed: [(u32, &str, bool); 2] = [
            (1, GOOD, false),
            (2, "ALTER TABLE t ADD COLUMN extra TEXT;", false),
        ];
        migrate_through(&conn, &fixed).unwrap();
        assert_eq!(user_version(&conn), 2);
        assert!(conn.prepare("SELECT extra FROM t").is_ok());
    }

    /// `self_tx` 标错的方向是不对称的：该标 true 却标了 false，会在用户启动时才炸
    /// （"cannot start a transaction within a transaction"）；反过来只是少包一层。
    /// 所以这条用文本判据把标志钉住，加迁移时忘了标会被测试拦下，而不是靠线上炸。
    #[test]
    fn self_tx_flag_matches_the_sql() {
        for (target, sql, self_tx) in MIGRATIONS {
            let has_tx = sql.lines().any(|l| {
                let t = l.trim().trim_end_matches(';').to_uppercase();
                t == "BEGIN" || t.starts_with("BEGIN ")
            }) && sql.lines().any(|l| {
                let t = l.trim().trim_end_matches(';').to_uppercase();
                t == "COMMIT" || t == "END"
            });
            assert_eq!(
                *self_tx, has_tx,
                "v{target} 的事务自述与 SQL 实际不符（实际自带事务={has_tx}）"
            );
        }
    }

    /// `LATEST_VERSION` 是手写常量，`MIGRATIONS` 末元素才是事实。两者漂移会让
    /// 新迁移被静默跳过（常量写小了）或让健康库被判为未迁完（写大了）。
    #[test]
    fn latest_version_constant_tracks_migrations_table() {
        assert_eq!(
            LATEST_VERSION,
            expected_version(),
            "加了迁移步骤却忘了同步 LATEST_VERSION"
        );
    }

    #[test]
    fn needs_migration_tracks_version() {
        let conn = crate::db::test_conn();
        assert!(!needs_migration(&conn).unwrap(), "已到最新不该再迁移");
        conn.execute_batch("PRAGMA user_version = 6;").unwrap();
        assert!(needs_migration(&conn).unwrap(), "落后一步就该报需要迁移");
    }
}
