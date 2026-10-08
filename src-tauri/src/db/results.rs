//! processing_results：AI 结果版本化写入与人工修正（03 文档 §3.3；02 清单 §2.5）。
//! 同一片段仅一行 superseded=0（最新）；retry 产生新版本，旧版本保留供对比。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{fragments, now_iso};
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedLink {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingResult {
    pub id: String,
    pub version: i64,
    pub category: String,
    pub subcategory: Option<String>,
    pub category_manual: bool,
    pub tags: Vec<String>,
    pub tags_manual: bool,
    pub summary: String,
    pub links: Vec<ExtractedLink>,
    pub degraded: bool,
    pub model_used: Option<String>,
    pub processed_at: String,
}

const SEL_COLS: &str = "id, version, category, subcategory, category_manual, tags, tags_manual, \
                        summary, links, degraded, model_used, processed_at";

fn row_to_result(r: &rusqlite::Row) -> rusqlite::Result<ProcessingResult> {
    let tags_json: String = r.get(5)?;
    let links_json: String = r.get(8)?;
    Ok(ProcessingResult {
        id: r.get(0)?,
        version: r.get(1)?,
        category: r.get(2)?,
        subcategory: r.get(3)?,
        category_manual: r.get::<_, i64>(4)? != 0,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        tags_manual: r.get::<_, i64>(6)? != 0,
        summary: r.get(7)?,
        links: serde_json::from_str(&links_json).unwrap_or_default(),
        degraded: r.get::<_, i64>(9)? != 0,
        model_used: r.get(10)?,
        processed_at: r.get(11)?,
    })
}

/// 最新未废弃结果（每片段至多一行，由 insert_result 维护）。
pub fn latest(conn: &Connection, fragment_id: &str) -> Result<Option<ProcessingResult>, AppError> {
    conn.query_row(
        &format!(
            "SELECT {SEL_COLS} FROM processing_results
              WHERE fragment_id = ?1 AND superseded = 0
              ORDER BY version DESC LIMIT 1"
        ),
        params![fragment_id],
        row_to_result,
    )
    .optional()
    .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 历史版本（superseded=1），供详情页 priorResults 版本切换（02 §2.2）。
pub fn superseded_versions(
    conn: &Connection,
    fragment_id: &str,
) -> Result<Vec<ProcessingResult>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SEL_COLS} FROM processing_results
              WHERE fragment_id = ?1 AND superseded = 1
              ORDER BY version DESC"
        ))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map(params![fragment_id], row_to_result)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| AppError::DbRead(e.to_string()))
}

/// pipeline 完成时写入新版本并废弃旧版本（02 §2.3 行为：旧结果标记 superseded）。
/// category 须为 04 文档 §1 一级枚举（写边界校验，防脏值进筛选器）。
pub fn insert_result(
    conn: &Connection,
    fragment_id: &str,
    category: &str,
    subcategory: Option<&str>,
    tags: &[String],
    summary: &str,
    links: &[ExtractedLink],
    degraded: bool,
    model_used: Option<&str>,
) -> Result<ProcessingResult, AppError> {
    const CATEGORIES: [&str; 8] = ["技术", "产品", "商业", "学习", "生活", "资讯", "创意", "其他"];
    if !CATEGORIES.contains(&category) {
        return Err(AppError::InputInvalid(format!("category: {category}")));
    }
    if fragments::get(conn, fragment_id)?.is_none() {
        return Err(AppError::NotFound);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    let tags_json = serde_json::to_string(tags).map_err(|e| AppError::Internal(e.to_string()))?;
    let links_json =
        serde_json::to_string(links).map_err(|e| AppError::Internal(e.to_string()))?;
    let tx = conn.unchecked_transaction()?;
    let version: i64 = tx.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM processing_results WHERE fragment_id = ?1",
        params![fragment_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE processing_results SET superseded = 1
          WHERE fragment_id = ?1 AND superseded = 0",
        params![fragment_id],
    )?;
    tx.execute(
        "INSERT INTO processing_results
           (id, fragment_id, version, superseded, category, subcategory, tags, summary, links,
            degraded, model_used, processed_at)
         VALUES (?1,?2,?3,0,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![&id, fragment_id, version, category, subcategory, &tags_json, summary,
                &links_json, degraded as i64, model_used, &now],
    )?;
    // FTS 同步与结果写入同事务：崩溃不会留下"有新结果但索引仍旧"的失步态
    fragments::fts_sync_tags(&tx, fragment_id)?;
    tx.commit()?;
    latest(conn, fragment_id)?.ok_or(AppError::Internal("写入后读取失败".into()))
}

/// 人工修正分类（02 §2.5）：原地改最新未废弃版本并打 manual 标；无结果时新建一行。
/// 后续 retry 写新版本时须沿用 manual 字段（agent 层职责，P5 实现）。
pub fn set_category_manual(
    conn: &Connection,
    fragment_id: &str,
    category: &str,
    subcategory: Option<&str>,
) -> Result<ProcessingResult, AppError> {
    const CATEGORIES: [&str; 8] = ["技术", "产品", "商业", "学习", "生活", "资讯", "创意", "其他"];
    if !CATEGORIES.contains(&category) {
        return Err(AppError::InputInvalid(format!("category: {category}")));
    }
    match latest(conn, fragment_id)? {
        Some(res) => {
            conn.execute(
                "UPDATE processing_results SET category = ?2, subcategory = ?3, category_manual = 1
                  WHERE id = ?1",
                params![res.id, category, subcategory],
            )?;
        }
        None => {
            insert_result(conn, fragment_id, category, subcategory, &[], "", &[], false, None)?;
            conn.execute(
                "UPDATE processing_results SET category_manual = 1
                  WHERE fragment_id = ?1 AND superseded = 0",
                params![fragment_id],
            )?;
        }
    }
    latest(conn, fragment_id)?.ok_or(AppError::NotFound)
}

/// 人工修正标签：同上，原地改并打 manual 标。
pub fn set_tags_manual(
    conn: &Connection,
    fragment_id: &str,
    tags: &[String],
) -> Result<ProcessingResult, AppError> {
    if tags.len() > 8 || tags.iter().any(|t| t.is_empty() || t.chars().count() > 16) {
        return Err(AppError::InputInvalid("标签需 1–16 字且不超过 8 个".into()));
    }
    let tags_json = serde_json::to_string(tags).map_err(|e| AppError::Internal(e.to_string()))?;
    match latest(conn, fragment_id)? {
        Some(res) => {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "UPDATE processing_results SET tags = ?2, tags_manual = 1 WHERE id = ?1",
                params![res.id, &tags_json],
            )?;
            fragments::fts_sync_tags(&tx, fragment_id)?; // 与 FTS 同步同事务
            tx.commit()?;
        }
        None => {
            // insert_result 内部已同步 FTS（含新 tags）
            insert_result(conn, fragment_id, "其他", None, tags, "", &[], false, None)?;
            conn.execute(
                "UPDATE processing_results SET tags_manual = 1
                  WHERE fragment_id = ?1 AND superseded = 0",
                params![fragment_id],
            )?;
        }
    }
    latest(conn, fragment_id)?.ok_or(AppError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fragments::NewFragment;

    fn frag(conn: &Connection) -> String {
        let text = format!("结果表测试片段内容 {}", uuid::Uuid::new_v4());
        fragments::insert(
            conn,
            &NewFragment { content: &text, title: None, source: "manual", external_url: None, media_type: None, note: None, media_path: None, id: None },
        )
        .unwrap()
        .id
    }

    #[test]
    fn versions_superseded_in_order() {
        let conn = crate::db::test_conn();
        let id = frag(&conn);
        let v1 = insert_result(&conn, &id, "技术", Some("数据库"), &["SQLite".into()], "摘要一",
            &[ExtractedLink { url: "https://a.cn".into(), text: None }], false, Some("m1")).unwrap();
        assert_eq!(v1.version, 1);
        let v2 = insert_result(&conn, &id, "学习", None, &[], "摘要二", &[], true, None).unwrap();
        assert_eq!(v2.version, 2);
        assert_eq!(v2.category, "学习");
        assert!(v2.degraded);
        // 旧版本保留但 superseded=1
        let old: i64 = conn.query_row(
            "SELECT superseded FROM processing_results WHERE id = ?1", params![v1.id], |r| r.get(0)).unwrap();
        assert_eq!(old, 1);
        assert_eq!(latest(&conn, &id).unwrap().unwrap().version, 2);
        assert_eq!(latest(&conn, &id).unwrap().unwrap().links.len(), 0);
        // v1 的 links 原样可读
        let links: String = conn.query_row(
            "SELECT links FROM processing_results WHERE id = ?1", params![v1.id], |r| r.get(0)).unwrap();
        assert!(links.contains("https://a.cn"));
    }

    #[test]
    fn rejects_bad_category_and_missing_fragment() {
        let conn = crate::db::test_conn();
        let id = frag(&conn);
        assert!(matches!(
            insert_result(&conn, &id, "玄学", None, &[], "", &[], false, None),
            Err(AppError::InputInvalid(_))
        ));
        assert!(matches!(
            insert_result(&conn, "ghost-id", "技术", None, &[], "", &[], false, None),
            Err(AppError::NotFound)
        ));
    }

    #[test]
    fn manual_flags_survive_and_validate() {
        let conn = crate::db::test_conn();
        let id = frag(&conn);
        insert_result(&conn, &id, "技术", Some("后端"), &["Rust".into()], "摘要", &[], false, None).unwrap();
        let r = set_category_manual(&conn, &id, "生活", None).unwrap();
        assert!(r.category_manual && r.version == 1); // 原地改，不产生新版本
        let r = set_tags_manual(&conn, &id, &["家居".into(), "收纳".into()]).unwrap();
        assert!(r.tags_manual && r.tags == vec!["家居".to_string(), "收纳".to_string()]);
        // 无效标签被拒
        assert!(set_tags_manual(&conn, &id, &["".into()]).is_err());
        assert!(set_tags_manual(&conn, &id, &["超".repeat(20)]).is_err());
        // 新版本沿用：AI 再跑会覆盖非 manual 列，manual 沿用由 agent 层负责（P5）；
        // 无结果片段直接人工修正会新建 v1
        let id2 = frag(&conn);
        let r = set_category_manual(&conn, &id2, "创意", Some("文案")).unwrap();
        assert_eq!((r.version, r.category_manual), (1, true));
        // manual tags 同步进 FTS
        set_tags_manual(&conn, &id2, &["节日文案".into()]).unwrap();
        let hits = fragments::fts_search(&conn, "\"节日文案\"", 10, &[]).unwrap();
        assert_eq!(hits[0].0, id2);
    }
}
