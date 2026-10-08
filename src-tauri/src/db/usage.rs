//! `llm_calls` 的读写面（批次17，表结构以 03 §3.10 为契约源）。
//! 写入由 `llm` 层的回执驱动；读取只给统计面板，按天/任务/模型三个维度分组。
//! 时间维度：**窗口**与**按天分组**都用本机日历日（`date(...,'localtime')`）——
//! 曾经窗口是"近 N×24 小时"而分组是本机日，于是首桶只剩几小时的残日，
//! 图上标着 `2026-09-22` 其实只覆盖那天 22:40 之后，标签说得像一整天、数据不是一整天。

use chrono::{Local, SecondsFormat, TimeZone};
use rusqlite::{params, Connection};

use crate::dto::usage::{UsageBucket, UsageStats};
use crate::error::AppError;
use crate::llm::usage::{CallReceipt, UsageRecorder};

use super::now_iso;

/// 一次调用落库。`called_at` 由这里统一取当前 UTC 时刻——回执不带时间戳，
/// 免得上层各自造时间，同一批数据出现两套时间口径。
pub fn insert_call(conn: &Connection, r: &CallReceipt) -> Result<(), AppError> {
    let (tin, tout) = match r.usage {
        Some(u) => (u.prompt_tokens, u.completion_tokens),
        None => (None, None),
    };
    conn.execute(
        "INSERT INTO llm_calls(called_at, endpoint, task, model, tokens_in, tokens_out, ok, error_code, latency_ms)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            now_iso(),
            r.endpoint,
            r.task,
            r.model,
            tin,
            tout,
            r.ok as i64,
            r.error_code,
            r.latency_ms.max(0),
        ],
    )
    .map_err(|e| AppError::DbWrite(e.to_string()))?;
    Ok(())
}

/// 把 llm 层的回执接到本表。计量是**尽力而为**：拿不到连接或写失败都绝不影响调用本身——
/// 用户要的是一次能用的会话，不是一条因为统计表写不进去就失败的流水线。
/// 持**连接池**而非连接：`run_skill` 把 router 移进独立线程，实现必须 `Send + Sync`；
/// 每次计量取一个连接（网络往返远比它贵，不构成瓶颈）。
#[derive(Clone)]
pub struct DbUsageRecorder {
    pub pool: super::Pool,
}

impl UsageRecorder for DbUsageRecorder {
    fn record(&self, receipt: &CallReceipt) {
        if let Ok(conn) = self.pool.get() {
            let _ = insert_call(&conn, receipt);
        }
    }
}

/// 窗口下界谓词：`?1` 绑 `Option<&str>`，`None` = 全量（此时逐行比较恒真，走全表）。
/// 比较的是**本机日历日**（与按天分组同一口径），所以 `近 7 天` 就是含今天在内的 7 个整日。
/// 本表只存计量、行数量级为"调用次数"，`date()` 让下界无法走索引也可接受。
const WINDOW: &str = "WHERE (?1 IS NULL OR date(called_at, 'localtime') >= ?1)";

/// 窗口下界的日历日；`range_days <= 0` = 全量（`None`）。
fn local_since(range_days: i64) -> Option<String> {
    if range_days <= 0 {
        return None;
    }
    let day = Local::now().naive_local().date() - chrono::Duration::days(range_days - 1);
    Some(day.format("%Y-%m-%d").to_string())
}

/// 六个聚合列，顺序与 `map_bucket` 一致。空集时 `SUM` 返回 NULL，故一律 COALESCE 成 0。
const AGGS: &str = "COALESCE(SUM(CASE WHEN ok = 0 THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(COALESCE(tokens_in, 0)), 0),
           COALESCE(SUM(COALESCE(tokens_out, 0)), 0),
           COALESCE(SUM(CASE WHEN tokens_in IS NULL AND tokens_out IS NULL THEN 1 ELSE 0 END), 0),
           CAST(COALESCE(AVG(latency_ms), 0) AS INTEGER),
           COALESCE(SUM(CASE WHEN endpoint = 'chat' THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN endpoint = 'embedding' THEN 1 ELSE 0 END), 0)";

fn read_aggs(r: &rusqlite::Row<'_>, base: usize) -> (i64, i64, i64, i64, i64, i64, i64) {
    (
        r.get(base).unwrap_or(0),
        r.get(base + 1).unwrap_or(0),
        r.get(base + 2).unwrap_or(0),
        r.get(base + 3).unwrap_or(0),
        r.get(base + 4).unwrap_or(0),
        r.get(base + 5).unwrap_or(0),
        r.get(base + 6).unwrap_or(0),
    )
}

fn map_bucket(r: &rusqlite::Row<'_>) -> rusqlite::Result<UsageBucket> {
    // 列序：k=0，COUNT(*)=1，其余七个聚合列从 2 起（与 AGGS 同序）。
    let (failures, tin, tout, unknown, avg, _chat, _emb) = read_aggs(r, 2);
    Ok(UsageBucket {
        key: r.get(0)?,
        calls: r.get(1)?,
        failures,
        tokens_in: tin,
        tokens_out: tout,
        tokens_unknown: unknown,
        avg_latency_ms: avg,
    })
}

/// 一个维度的分组统计。`key_expr` / `order` 只接本文件字面量（内部拼 SQL，
/// 外部输入一律走绑定参数——窗口下界是 `?1`，天数本身不进 SQL 文本）。
fn buckets(
    conn: &Connection,
    key_expr: &str,
    since: Option<&str>,
    order: &str,
) -> Result<Vec<UsageBucket>, AppError> {
    let sql = format!(
        "SELECT {key} AS k, COUNT(*), {aggs} FROM llm_calls {where_} GROUP BY k ORDER BY {order}",
        key = key_expr,
        aggs = AGGS,
        where_ = WINDOW,
        order = order,
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![since], map_bucket)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| AppError::DbRead(e.to_string()))?);
    }
    Ok(out)
}

/// `get_usage_stats` 的数据面。`range_days <= 0` = 全部历史。
/// 单价/金额不在这里算——后端不知道用户给每个端点定的价（02 §7.5）。
pub fn stats(conn: &Connection, range_days: i64) -> Result<UsageStats, AppError> {
    let window = local_since(range_days);
    let since = window.as_deref();

    let sql = format!("SELECT COUNT(*), {AGGS} FROM llm_calls {WINDOW}");
    let totals = conn
        .query_row(&sql, rusqlite::params![since], |r| {
            let calls: i64 = r.get(0).unwrap_or(0);
            let aggs = read_aggs(r, 1);
            Ok((calls, aggs))
        })
        .map_err(|e| AppError::DbRead(e.to_string()))?;

    // 全库最早一条，**不受窗口限制**：空态文案据此说话（"从未调用过" ≠ "近 N 天没有"）。
    let first: Option<String> = conn
        .query_row("SELECT MIN(called_at) FROM llm_calls", [], |r| r.get(0))
        .unwrap_or(None);

    let (failures, tin, tout, unknown, avg, chat, emb) = totals.1;
    Ok(UsageStats {
        range_days,
        total_calls: totals.0,
        total_failures: failures,
        tokens_in: tin,
        tokens_out: tout,
        tokens_unknown: unknown,
        avg_latency_ms: avg,
        chat_calls: chat,
        embedding_calls: emb,
        by_day: buckets(conn, "date(called_at, 'localtime')", since, "k ASC")?,
        by_task: buckets(conn, "task", since, "COUNT(*) DESC, k ASC")?,
        by_model: buckets(conn, "model", since, "COUNT(*) DESC, k ASC")?,
        first_recorded_at: first,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::usage::{NoUsageRecorder, Usage};

    fn receipt(endpoint: &'static str, task: &str, model: &str, usage: Option<Usage>) -> CallReceipt {
        CallReceipt {
            endpoint,
            task: task.into(),
            model: model.into(),
            usage,
            ok: true,
            error_code: None,
            latency_ms: 100,
        }
    }

    fn usage(pin: i64, pout: i64) -> Option<Usage> {
        Some(Usage { prompt_tokens: Some(pin), completion_tokens: Some(pout) })
    }

    #[test]
    fn insert_then_stats_totals_and_buckets() {
        let conn = crate::db::test_conn();
        insert_call(&conn, &receipt("chat", "classify", "qwen3-max", usage(120, 30))).unwrap();
        insert_call(&conn, &receipt("chat", "summarize", "qwen3-max", usage(80, 40))).unwrap();
        insert_call(&conn, &receipt("embedding", "embed", "text-embedding-v3", Some(Usage { prompt_tokens: Some(7), completion_tokens: None }))).unwrap();

        let s = stats(&conn, 0).unwrap();
        assert_eq!((s.total_calls, s.chat_calls, s.embedding_calls), (3, 2, 1));
        assert_eq!((s.tokens_in, s.tokens_out), (207, 70));
        assert_eq!(s.tokens_unknown, 0);
        assert_eq!(s.by_task.len(), 3);
        assert_eq!(s.by_model.len(), 2);
        assert_eq!(s.by_day.len(), 1, "同一天三条应归一桶");
        assert_eq!(s.by_day[0].calls, 3);
        // date(...,'localtime') 真的算出了日历日（返回 NULL 时这里会拿到空串/报错）
        assert_eq!(s.by_day[0].key.len(), 10, "按天桶键应是 YYYY-MM-DD");
        assert!(s.first_recorded_at.is_some());
    }

    #[test]
    fn missing_usage_is_counted_as_unknown_not_zero() {
        let conn = crate::db::test_conn();
        insert_call(&conn, &receipt("chat", "tag", "m", None)).unwrap();
        insert_call(&conn, &receipt("chat", "tag", "m", usage(50, 5))).unwrap();
        let s = stats(&conn, 0).unwrap();
        assert_eq!(s.tokens_in, 50);
        assert_eq!(s.tokens_unknown, 1, "未知必须留痕，否则合计看着像精确值");
        let tag = s.by_task.iter().find(|b| b.key == "tag").unwrap();
        assert_eq!((tag.calls, tag.tokens_unknown), (2, 1));
    }

    #[test]
    fn failures_are_counted_separately() {
        let conn = crate::db::test_conn();
        insert_call(
            &conn,
            &CallReceipt {
                ok: false,
                error_code: Some("E_LLM_TIMEOUT".into()),
                latency_ms: 900,
                ..receipt("chat", "classify", "m", None)
            },
        )
        .unwrap();
        let s = stats(&conn, 0).unwrap();
        assert_eq!((s.total_calls, s.total_failures), (1, 1));
        assert_eq!(s.by_task[0].failures, 1);
    }

    #[test]
    fn window_excludes_rows_older_than_range() {
        let conn = crate::db::test_conn();
        conn.execute(
            "INSERT INTO llm_calls(called_at, endpoint, task, model, ok, latency_ms)
             VALUES('2020-01-01T00:00:00.000Z','chat','classify','m',1,10)",
            [],
        )
        .unwrap();
        insert_call(&conn, &receipt("chat", "classify", "m", None)).unwrap();
        let s = stats(&conn, 7).unwrap();
        assert_eq!(s.total_calls, 1, "七年前的记录不该进近 7 天窗口");
        assert_eq!(stats(&conn, 0).unwrap().total_calls, 2, "全量窗口应含历史");
        assert_eq!(s.first_recorded_at.as_deref(), Some("2020-01-01T00:00:00.000Z"));
    }

    #[test]
    fn window_is_aligned_to_local_calendar_days() {
        // 「近 7 天」＝含今天在内的 7 个**整日**，不是滚动 168 小时。用本机正午造时间戳，
        // 避开 UTC±14 跨界带来的抖动（真跨界的行本来就该按本机日归属）。
        let conn = crate::db::test_conn();
        let at_noon_local = |back: i64| {
            let day = Local::now().naive_local().date() - chrono::Duration::days(back);
            let naive = day.and_hms_opt(12, 0, 0).unwrap();
            TimeZone::from_local_datetime(&Local, &naive)
                .single()
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        };
        let ins = |back: i64| {
            conn.execute(
                "INSERT INTO llm_calls(called_at, endpoint, task, model, ok, latency_ms)
                 VALUES(?1,'chat','classify','m',1,10)",
                params![at_noon_local(back)],
            )
            .unwrap()
        };
        ins(6);
        ins(7);
        let s = stats(&conn, 7).unwrap();
        assert_eq!(s.total_calls, 1, "第 7 天前那天（back=6）在窗口内，back=7 应落在窗外");
        assert_eq!(s.by_day.len(), 1);
        assert_eq!(stats(&conn, 8).unwrap().total_calls, 2, "多要一天就该把那条收进来");
    }

    #[test]
    fn empty_db_yields_zeroes_and_no_first_record() {
        let conn = crate::db::test_conn();
        let s = stats(&conn, 30).unwrap();
        assert_eq!(s.total_calls, 0);
        assert_eq!((s.total_failures, s.tokens_in, s.avg_latency_ms, s.chat_calls), (0, 0, 0, 0));
        assert!(s.by_day.is_empty() && s.by_task.is_empty() && s.by_model.is_empty());
        assert!(s.first_recorded_at.is_none());
    }

    #[test]
    fn recorder_impl_writes_and_noop_impl_does_not() {
        // 记录器走连接池（要能跨线程），故用临时文件库建池，而不是内存里的单连接。
        let dir = std::env::temp_dir().join(format!("sc-usage-test-{}", uuid::Uuid::new_v4()));
        let pool = crate::db::init_pool(&dir.join("smart.db")).unwrap();
        let conn = pool.get().unwrap();
        DbUsageRecorder { pool: pool.clone() }.record(&receipt("chat", "skill_run", "m", None));
        assert_eq!(stats(&conn, 0).unwrap().total_calls, 1);
        NoUsageRecorder.record(&receipt("chat", "skill_run", "m", None));
        assert_eq!(stats(&conn, 0).unwrap().total_calls, 1, "NoUsageRecorder 不该落库");
        drop(conn);
        drop(pool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn llm_calls_has_no_link_to_fragment_content() {
        // 契约红线：统计表面向"调用元数据"，不与 fragments 关联——
        // 否则一次真删除会改写历史统计，且面板有变成第二份原文副本的风险。
        let ddl: String = crate::db::test_conn()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='llm_calls'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!ddl.contains("fragment_id"));
        assert!(!ddl.contains("REFERENCES"));
        assert!(ddl.contains("tokens_in INTEGER"));
    }
}
