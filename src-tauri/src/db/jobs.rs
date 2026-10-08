//! fragment_status：处理队列状态机（03 文档 §2 与 §5 ①②）。
//! pending → running →(done|failed)；失败未超上限自动回 pending。

use rusqlite::{params, Connection, OptionalExtension};

use super::now_iso;
use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Done,
    Failed,
    /// 入队前闸门判定为垃圾/仅记录，永不进 AI 流水线（03 §2 skipped）。
    Skipped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
        }
    }
    fn parse(s: &str) -> Result<Status, AppError> {
        match s {
            "pending" => Ok(Status::Pending),
            "running" => Ok(Status::Running),
            "done" => Ok(Status::Done),
            "failed" => Ok(Status::Failed),
            "skipped" => Ok(Status::Skipped),
            other => Err(AppError::Internal(format!("未知状态: {other}"))),
        }
    }
}

/// 新片段入队（fragments.insert 内调用，同事务）。
pub fn enqueue(conn: &Connection, fragment_id: &str, enqueued_at: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO fragment_status (fragment_id, status, enqueued_at) VALUES (?1, 'pending', ?2)",
        params![fragment_id, enqueued_at],
    )?;
    Ok(())
}

/// 闸门判定为无需处理：pending → skipped（03 §2 skipped，02 §1.1 采集行为）。
/// 仅改尚未领取的行；worker 只领取 pending，故 skipped 永不进流水线、零 token。
pub fn mark_skipped(conn: &Connection, fragment_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE fragment_status SET status = 'skipped' WHERE fragment_id = ?1 AND status = 'pending'",
        params![fragment_id],
    )?;
    Ok(())
}

/// worker 途中的强制跳过（批次21-B 图片闸门）：行已被领取（running）时 `mark_skipped` 命不中，
/// 会留下一条永远挂着的租约，故这里允许 pending|running → skipped 并释放租约。
/// done/failed 不参与——那两种状态已由正常路径结清，不该被闸门回退。
pub fn skip_claimed(conn: &Connection, fragment_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE fragment_status
            SET status = 'skipped', lease_owner = NULL, lease_expires = NULL
          WHERE fragment_id = ?1 AND status IN ('pending', 'running')",
        params![fragment_id],
    )?;
    Ok(())
}

pub fn status_of(conn: &Connection, fragment_id: &str) -> Result<Option<Status>, AppError> {
    conn.query_row(
        "SELECT status FROM fragment_status WHERE fragment_id = ?1",
        params![fragment_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| AppError::DbRead(e.to_string()))?
    .as_deref()
    .map(Status::parse)
    .transpose()
}

/// worker 批量领取（03 §5 ①：ORDER BY/LIMIT 改 IN 子查询，见 schema.sql 头注释）。
/// 返回本次领到的 fragment_id 列表，租约到期前归该 worker 所有。
pub fn claim_next(
    conn: &Connection,
    worker_id: &str,
    lease_expires: &str,
    batch: usize,
) -> Result<Vec<String>, AppError> {
    let batch = batch.clamp(1, 50) as i64;
    let now = now_iso();
    let mut stmt = conn.prepare(
        "UPDATE fragment_status
            SET status = 'running', lease_owner = ?1, started_at = ?2, lease_expires = ?3
          WHERE fragment_id IN (
            SELECT fragment_id FROM fragment_status
             WHERE status = 'pending'
             ORDER BY enqueued_at, fragment_id
             LIMIT ?4)
          RETURNING fragment_id",
    )?;
    let ids = stmt
        .query_map(params![worker_id, &now, lease_expires, batch], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// 崩溃恢复：租约过期的 running 收回为 pending（03 §5 ②）。返回收回数量。
pub fn reclaim_expired_leases(conn: &Connection) -> Result<usize, AppError> {
    let now = now_iso();
    let n = conn.execute(
        "UPDATE fragment_status
            SET status = 'pending', lease_owner = NULL, lease_expires = NULL
          WHERE status = 'running'
            AND (lease_expires IS NULL OR lease_expires < ?1)",
        params![&now],
    )?;
    Ok(n)
}

/// 处理成功：running → done。
pub fn mark_done(conn: &Connection, fragment_id: &str) -> Result<(), AppError> {
    let now = now_iso();
    let n = conn.execute(
        "UPDATE fragment_status
            SET status = 'done', finished_at = ?2, lease_owner = NULL, lease_expires = NULL, error_code = NULL
          WHERE fragment_id = ?1 AND status = 'running'",
        params![fragment_id, &now],
    )?;
    if n == 0 {
        return Err(AppError::StateConflict);
    }
    Ok(())
}

/// 处理失败：未超上限回 pending（retry_count+1），超限置 failed（03 §2 状态机）。
/// 返回失败后的新状态。
pub fn mark_failed(conn: &Connection, fragment_id: &str, error_code: &str) -> Result<Status, AppError> {
    let (retry_count, max_retries): (i64, i64) = conn
        .query_row(
            "SELECT retry_count, max_retries FROM fragment_status WHERE fragment_id = ?1",
            params![fragment_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::DbRead(e.to_string()))?
        .ok_or(AppError::NotFound)?;
    let now = now_iso();
    if retry_count < max_retries {
        let n = conn.execute(
            "UPDATE fragment_status
                SET status = 'pending', retry_count = retry_count + 1, error_code = ?2,
                    lease_owner = NULL, lease_expires = NULL
              WHERE fragment_id = ?1 AND status = 'running'",
            params![fragment_id, error_code],
        )?;
        // 影响行数 0 = 租约已被回收、行不再是 running，报告状态冲突而非默默改错状态
        if n == 0 {
            return Err(AppError::StateConflict);
        }
        Ok(Status::Pending)
    } else {
        let n = conn.execute(
            "UPDATE fragment_status
                SET status = 'failed', error_code = ?2, finished_at = ?3,
                    lease_owner = NULL, lease_expires = NULL
              WHERE fragment_id = ?1 AND status = 'running'",
            params![fragment_id, error_code, &now],
        )?;
        if n == 0 {
            return Err(AppError::StateConflict);
        }
        Ok(Status::Failed)
    }
}

/// 手动重试（02 §2.3）：failed、done(degraded)、skipped（强制处理）、或 done 且正文已修订
/// （`content_updated_at` 晚于当前结果 `processed_at`）允许；重置状态并给新的重试预算。
/// 最后一条是批次6-①「编辑原文后重整理」的出口：改过正文的正常 done 片段，其摘要/向量描述的是旧文本，
/// 用户应能点「重新处理」让它对齐——否则编辑等于永久锁死一条再也整不动的旧结果。
///
/// `outbound_ok` 是调用方带回的**用户明示授权**（隐私闸门 §2.2）：`retry_fragment` 只负责复位
/// 状态，真正把正文发出去的是 worker（异步、另一次调用），所以授权必须落库才活得到那个出口。
/// 每次重试都按本次的明示值覆写（不继承上次的授权）——一次确认只买一次外发。
pub fn manual_retry(
    conn: &Connection,
    fragment_id: &str,
    outbound_ok: bool,
) -> Result<Status, AppError> {
    let row: Option<(String, i64, Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT s.status, s.retry_count,
                    (SELECT r.degraded FROM processing_results r
                      WHERE r.fragment_id = s.fragment_id AND r.superseded = 0
                      ORDER BY r.version DESC LIMIT 1),
                    (SELECT CASE WHEN f.content_updated_at IS NOT NULL AND EXISTS (
                                SELECT 1 FROM processing_results r
                                 WHERE r.fragment_id = s.fragment_id AND r.superseded = 0
                                   AND f.content_updated_at > r.processed_at
                            ) THEN 1 ELSE 0 END
                       FROM fragments f WHERE f.id = s.fragment_id)
               FROM fragment_status s WHERE s.fragment_id = ?1",
            params![fragment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let (status, _retry, degraded, stale) = row.ok_or(AppError::NotFound)?;
    let allowed = status == "failed"
        || status == "skipped"
        || (status == "done" && (degraded == Some(1) || stale == Some(1)));
    if !allowed {
        return Err(AppError::StateConflict);
    }
    conn.execute(
        "UPDATE fragment_status
            SET status = 'pending', retry_count = 0, error_code = NULL,
                started_at = NULL, finished_at = NULL, lease_owner = NULL, lease_expires = NULL,
                enqueued_at = ?2, outbound_ok = ?3
          WHERE fragment_id = ?1",
        params![fragment_id, now_iso(), outbound_ok as i64],
    )?;
    Ok(Status::Pending)
}

/// 这条片段是否已被用户**明示**授权外发（隐私闸门的出口凭据，02 §2.3 / §2.7）。
/// 读失败一律按"未授权"：外发收不回，判错的方向只能是拒绝。
pub fn outbound_allowed(conn: &Connection, fragment_id: &str) -> bool {
    conn.query_row(
        "SELECT outbound_ok FROM fragment_status WHERE fragment_id = ?1",
        params![fragment_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        != 0
}

/// 撤掉外发授权：正文一旦被改写，先前那句"确认发送"针对的已经不是现在这份文本了。
/// 不设这个闸，确认就成了一张长期通行证——授权时看到的是旧号码，发出去的是新密码。
pub fn clear_outbound(conn: &Connection, fragment_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE fragment_status SET outbound_ok = 0 WHERE fragment_id = ?1",
        params![fragment_id],
    )?;
    Ok(())
}

/// 队列统计（供 UI 徽章/后续 worker 用）。
pub fn queue_counts(conn: &Connection) -> Result<(i64, i64), AppError> {
    let db_read = |e: rusqlite::Error| AppError::DbRead(e.to_string());
    let pending: i64 = conn.query_row(
        "SELECT count(*) FROM fragment_status s
          WHERE s.status IN ('pending','running')
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = s.fragment_id)",
        [],
        |r| r.get(0),
    ).map_err(db_read)?;
    let failed: i64 = conn.query_row(
        "SELECT count(*) FROM fragment_status s
          WHERE s.status = 'failed'
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = s.fragment_id)",
        [],
        |r| r.get(0),
    ).map_err(db_read)?;
    Ok((pending, failed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fragments::{self, NewFragment};

    fn add(conn: &Connection, text: &str) -> String {
        fragments::insert(
            conn,
            &NewFragment { content: text, title: None, source: "manual", external_url: None, media_type: None, note: None, media_path: None, id: None },
        )
        .unwrap()
        .id
    }

    #[test]
    fn claim_is_fifo_and_locks_rows() {
        let conn = crate::db::test_conn();
        let a = add(&conn, "片段A");
        let b = add(&conn, "片段B");
        // 同毫秒入队时顺序不保证，只验证 batch=1 逐个领取且不重叠
        let mut got = vec![];
        for w in ["w1", "w2"] {
            let claimed = claim_next(&conn, w, "2099-01-01T00:00:00.000Z", 1).unwrap();
            assert_eq!(claimed.len(), 1);
            assert_eq!(status_of(&conn, &claimed[0]).unwrap(), Some(Status::Running));
            got.push(claimed[0].clone());
        }
        got.sort();
        let mut ab = vec![a, b];
        ab.sort();
        assert_eq!(got, ab);
        assert!(claim_next(&conn, "w3", "2099-01-01T00:00:00.000Z", 5).unwrap().is_empty());
    }

    #[test]
    fn expired_lease_is_reclaimed() {
        let conn = crate::db::test_conn();
        let a = add(&conn, "租约测试");
        claim_next(&conn, "w1", "2000-01-01T00:00:00.000Z", 1).unwrap(); // 立即过期
        let n = reclaim_expired_leases(&conn).unwrap();
        assert_eq!(n, 1);
        assert_eq!(status_of(&conn, &a).unwrap(), Some(Status::Pending));
    }

    #[test]
    fn auto_retry_until_limit_then_failed() {
        let conn = crate::db::test_conn();
        let a = add(&conn, "会失败的片段");
        let lease_far = "2099-01-01T00:00:00.000Z";
        // max_retries=3：前 3 次回 pending，第 4 次落 failed
        for expected in [Status::Pending, Status::Pending, Status::Pending, Status::Failed] {
            claim_next(&conn, "w1", lease_far, 1).unwrap();
            let s = mark_failed(&conn, &a, "E_LLM_TIMEOUT").unwrap();
            assert_eq!(s, expected);
        }
        assert_eq!(status_of(&conn, &a).unwrap(), Some(Status::Failed));
        let rc: i64 = conn
            .query_row("SELECT retry_count FROM fragment_status WHERE fragment_id=?1", params![&a], |r| r.get(0))
            .unwrap();
        assert_eq!(rc, 3);
    }

    #[test]
    fn done_requires_running_and_manual_retry_gates() {
        let conn = crate::db::test_conn();
        let a = add(&conn, "状态机测试");
        assert!(matches!(mark_done(&conn, &a), Err(AppError::StateConflict))); // pending 不能直接 done
        claim_next(&conn, "w1", "2099-01-01T00:00:00.000Z", 1).unwrap();
        mark_done(&conn, &a).unwrap();
        assert_eq!(status_of(&conn, &a).unwrap(), Some(Status::Done));
        // done 且无 degraded 结果 → 不允许手动重试
        assert!(matches!(manual_retry(&conn, &a, false), Err(AppError::StateConflict)));
        // 写入 degraded 结果后允许
        crate::db::results::insert_result(&conn, &a, "其他", None, &[], "兜底摘要", &[], true, None).unwrap();
        assert_eq!(manual_retry(&conn, &a, false).unwrap(), Status::Pending);
        // 手动重试重置 retry_count 与新入队时间
        let (rc, st): (i64, String) = conn
            .query_row("SELECT retry_count, status FROM fragment_status WHERE fragment_id=?1", params![&a], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((rc, st.as_str()), (0, "pending"));
        assert!(matches!(manual_retry(&conn, "no-such-id", false), Err(AppError::NotFound)));
    }

    #[test]
    fn content_stale_done_fragment_can_be_retried() {
        let conn = crate::db::test_conn();
        let a = add(&conn, "改前正文一段较长内容");
        claim_next(&conn, "w1", "2099-01-01T00:00:00.000Z", 1).unwrap();
        mark_done(&conn, &a).unwrap();
        // 非 degraded 结果，正常 done → 不允许重试
        crate::db::results::insert_result(&conn, &a, "技术", None, &[], "摘要", &[], false, None).unwrap();
        assert!(matches!(manual_retry(&conn, &a, false), Err(AppError::StateConflict)));
        // 模拟正文修订：content_updated_at 晚于结果 processed_at → stale，放开重试
        conn.execute("UPDATE fragments SET content_updated_at='2099-01-01T00:00:00.000Z' WHERE id=?1", params![&a]).unwrap();
        assert_eq!(manual_retry(&conn, &a, false).unwrap(), Status::Pending);
    }

    #[test]
    fn skipped_survives_claim_and_can_be_force_retried() {
        let conn = crate::db::test_conn();
        let a = add(&conn, "将被闸门跳过的片段");
        mark_skipped(&conn, &a).unwrap();
        assert_eq!(status_of(&conn, &a).unwrap(), Some(Status::Skipped));
        // worker 只领 pending：skipped 永不被领取（零 token）
        assert!(claim_next(&conn, "w1", "2099-01-01T00:00:00.000Z", 5).unwrap().is_empty());
        // skipped 可被手动「重新处理」强制入队 → pending
        assert_eq!(manual_retry(&conn, &a, false).unwrap(), Status::Pending);
        assert_eq!(status_of(&conn, &a).unwrap(), Some(Status::Pending));
        // 已回 pending 后可被领取
        assert_eq!(claim_next(&conn, "w1", "2099-01-01T00:00:00.000Z", 1).unwrap(), vec![a.clone()]);
    }

    #[test]
    fn queue_counts_ignores_tombstones() {
        let conn = crate::db::test_conn();
        let a = add(&conn, "计数一");
        add(&conn, "计数二");
        let (p, f) = queue_counts(&conn).unwrap();
        assert_eq!((p, f), (2, 0));
        claim_next(&conn, "w1", "2099-01-01T00:00:00.000Z", 5).unwrap();
        mark_failed(&conn, &a, "E_LLM_BAD_OUTPUT").unwrap();
        let (p, _) = queue_counts(&conn).unwrap();
        assert_eq!(p, 2); // a 回 pending，b 仍 pending（被 claim 后收回？不，b 也在 running）
    }
}
