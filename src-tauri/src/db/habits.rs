//! habits 表读写（03 §3.9③，02 §7.3）。AI 归纳产出候选，用户须显式启用。
//! 本模块只做 CRUD 与状态迁移；归纳（PROMPT_HABIT_INFER）依赖 LLM，属后续。
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::now_iso;
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitRule {
    pub id: String,
    pub pattern: String,
    pub action: String,
    pub hits: i64,
    pub total: i64,
    pub state: String,
    pub recent_auto: i64,
    pub samples: Vec<String>,
}

fn row_to_habit(r: &rusqlite::Row) -> rusqlite::Result<HabitRule> {
    let samples_json: String = r.get(7)?;
    Ok(HabitRule {
        id: r.get(0)?,
        pattern: r.get(1)?,
        action: r.get(2)?,
        hits: r.get(3)?,
        total: r.get(4)?,
        state: r.get(5)?,
        recent_auto: r.get(6)?,
        samples: serde_json::from_str(&samples_json).unwrap_or_default(),
    })
}

const HABIT_COLS: &str = "id, pattern, action, hits, total, state, recent_auto, samples";

pub fn list(conn: &Connection) -> Result<Vec<HabitRule>, AppError> {
    let mut stmt = conn
        .prepare(&format!("SELECT {HABIT_COLS} FROM habits ORDER BY hits DESC, updated_at DESC"))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map([], row_to_habit)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| AppError::DbRead(e.to_string()))
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<HabitRule>, AppError> {
    conn.query_row(
        &format!("SELECT {HABIT_COLS} FROM habits WHERE id = ?1"),
        params![id],
        row_to_habit,
    )
    .optional()
    .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 状态迁移（02 §7.3）：candidate/active/paused。仅改 state，不触碰归纳字段。
pub fn set_state(conn: &Connection, id: &str, state: &str) -> Result<HabitRule, AppError> {
    if !matches!(state, "candidate" | "active" | "paused") {
        return Err(AppError::InputInvalid(format!("state: {state}")));
    }
    if get(conn, id)?.is_none() {
        return Err(AppError::NotFound);
    }
    conn.execute(
        "UPDATE habits SET state = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, state, now_iso()],
    )
    .map_err(|e| AppError::DbWrite(e.to_string()))?;
    get(conn, id)?.ok_or(AppError::NotFound)
}

/// 落一条归纳出的候选规则（供后续 habit_infer 及测试使用）。
pub fn upsert_candidate(
    conn: &Connection,
    id: &str,
    pattern: &str,
    action: &str,
    hits: i64,
    total: i64,
    samples: &[String],
) -> Result<HabitRule, AppError> {
    let now = now_iso();
    let samples_json = serde_json::to_string(samples).unwrap_or_else(|_| "[]".into());
    let existing = get(conn, id)?;
    if existing.is_some() {
        conn.execute(
            "UPDATE habits SET pattern=?2, action=?3, hits=?4, total=?5, samples=?6, updated_at=?7 WHERE id=?1",
            params![id, pattern, action, hits, total, &samples_json, &now],
        )
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
    } else {
        conn.execute(
            "INSERT INTO habits (id,pattern,action,hits,total,state,recent_auto,samples,updated_at)
             VALUES (?1,?2,?3,?4,?5,'candidate',0,?6,?7)",
            params![id, pattern, action, hits, total, &samples_json, &now],
        )
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
    }
    get(conn, id)?.ok_or(AppError::Internal("upsert_candidate 后读取失败".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_defaults_and_state_transition() {
        let conn = crate::db::test_conn();
        let h = upsert_candidate(&conn, "h1", "资讯类×链接来源", "丢弃", 3, 4, &["样本甲".into(), "样本乙".into()]).unwrap();
        assert_eq!(h.state, "candidate");
        assert_eq!(h.hits, 3);
        assert_eq!(h.samples.len(), 2);
        let active = set_state(&conn, "h1", "active").unwrap();
        assert_eq!(active.state, "active");
        assert!(matches!(set_state(&conn, "h1", "on"), Err(AppError::InputInvalid(_))));
        assert!(matches!(set_state(&conn, "nope", "active"), Err(AppError::NotFound)));
    }

    #[test]
    fn upsert_updates_but_keeps_state() {
        let conn = crate::db::test_conn();
        upsert_candidate(&conn, "h2", "p", "快速归档", 2, 2, &[]).unwrap();
        set_state(&conn, "h2", "paused").unwrap();
        let again = upsert_candidate(&conn, "h2", "p2", "快速归档", 5, 6, &[]).unwrap();
        assert_eq!(again.pattern, "p2");
        assert_eq!(again.hits, 5);
        assert_eq!(again.state, "paused"); // 归纳刷新不改用户已设状态
        assert_eq!(list(&conn).unwrap().len(), 1);
    }
}
