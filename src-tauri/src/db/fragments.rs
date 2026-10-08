//! fragments 表读写 + FTS 行同步（03 文档 §3.1/§3.7/§5）。
//! 所有写入路径集中在此，FTS 与主表由本模块保持一致（见 schema.sql 头注释）。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{jobs, triage, now_iso};
use crate::error::AppError;

pub const MAX_CHARS: usize = 200_000; // 02 清单 §1.1

fn hex_sha256(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 正文内容哈希（去重唯一索引键）。对外暴露供剪贴板主动收集等预判重复，
/// 与 `insert` 内部计算保持一致，避免两处哈希实现漂移。
pub fn hash_of(content: &str) -> String {
    hex_sha256(content)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fragment {
    pub id: String,
    pub content: String,
    pub title: Option<String>,
    pub title_manual: bool,
    pub source: String,
    pub content_hash: String,
    pub external_url: Option<String>,
    pub char_count: i64,
    pub created_at: String,
    pub updated_at: String,
    // UI v2（03 §3.9 / 02 §7.1）：三层分拣 + 附言
    pub layer: String,
    pub media_type: String,
    pub archived_by: Option<String>,
    pub reviewed: bool,
    pub trashed_at: Option<String>,
    pub note: Option<String>,
    // 批次6-①（03 §3.11 / 02 §2.5）：人工修订正文的时刻与修订账。
    // `content_updated_at` 与当前结果的 `processed_at` 比较即得「结果已过期」，**不存布尔位**。
    pub content_updated_at: Option<String>,
    /// 原始 JSON 数组文本（解析见 [`EditEntry`]）；与 `tags` 同样按 03 §7 不过度规范化。
    pub edit_log: String,
    /// 批次21-B（03 §3.12 / 02 §1.4）：图片原样收藏时落在 `media` 目录下的**相对文件名**。
    /// `None` = 不是图片（或图片文件已丢失）。绝不存绝对路径——搬家即失联。
    pub media_path: Option<String>,
}

/// 人工修订记录的一条（存 `fragments.edit_log`，渲染在详情页原文下方）。
/// 只记"改了哪个字段、字数怎么变、改前长什么样"，不记完整旧文——完整旧文等于第二份明文副本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEntry {
    pub at: String,
    /// `"content"` | `"note"`
    pub field: String,
    pub before_chars: i64,
    pub after_chars: i64,
    /// 改前正文前 40 字；命中隐私规则时只写占位词（见 [`EDIT_LOG_WITHHELD`]）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub excerpt: Option<String>,
}

/// 隐私条目的摘录占位词。02 §7.1 规定隐私标签"不含命中的内容片段"，
/// 而编辑记录若干脆省掉摘录，用户会以为是漏记——故留一句明说的占位。
pub const EDIT_LOG_WITHHELD: &str = "（含隐私，摘录已省略）";

/// 编辑记录上限：超过则丢最旧（防止反复编辑把一行 JSON 撑大）。
const EDIT_LOG_CAP: usize = 20;

/// 改前文本摘录：前 40 字；含隐私则给占位词，绝不落明文。
fn edit_excerpt(text: &str) -> String {
    if triage::detect_sensitive(text).is_some() {
        return EDIT_LOG_WITHHELD.to_string();
    }
    let head: String = text.chars().take(40).collect();
    let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
    format!("“{head}{ellipsis}”")
}

/// 往既有 JSON 数组尾部追加一条并截断到上限（纯函数，便于单测）。
fn push_edit(existing: &str, entry: EditEntry) -> Vec<EditEntry> {
    let mut log: Vec<EditEntry> =
        serde_json::from_str(existing).unwrap_or_default();
    log.push(entry);
    let overflow = log.len().saturating_sub(EDIT_LOG_CAP);
    log.drain(0..overflow);
    log
}

/// 修订账落库形态（序列失败不该阻断编辑动作本身：数据已改，账记不上是次要故障）。
fn edit_log_json(log: &[EditEntry]) -> String {
    serde_json::to_string(log).unwrap_or_else(|_| "[]".into())
}

const FRAGMENT_COLS: &str = "id, content, title, title_manual, source, content_hash, external_url, \
    char_count, created_at, updated_at, layer, media_type, archived_by, reviewed, trashed_at, note, \
    content_updated_at, edit_log, media_path";

fn row_to_fragment(r: &rusqlite::Row) -> rusqlite::Result<Fragment> {
    Ok(Fragment {
        id: r.get(0)?,
        content: r.get(1)?,
        title: r.get(2)?,
        title_manual: r.get::<_, i64>(3)? != 0,
        source: r.get(4)?,
        content_hash: r.get(5)?,
        external_url: r.get(6)?,
        char_count: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        layer: r.get(10)?,
        media_type: r.get(11)?,
        archived_by: r.get(12)?,
        reviewed: r.get::<_, i64>(13)? != 0,
        trashed_at: r.get(14)?,
        note: r.get(15)?,
        content_updated_at: r.get(16)?,
        edit_log: r.get(17)?,
        media_path: r.get(18)?,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentListItem {
    pub id: String,
    pub title: Option<String>,
    pub excerpt: String,
    pub status: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    pub layer: String,
    pub media_type: String,
    pub archived_by: Option<String>,
    /// 批次21-B：图片文件名（相对 media 目录）。卡片缩略图靠它拼 asset URL，非图片为 None。
    pub media_path: Option<String>,
    /// 隐私命中的类型标签（本地规则，见 `triage::detect_sensitive`；`None`=未命中）。
    /// 卡片要挂"注意保密"标签，而摘要只有 120 字，故列表查询另取前 4000 字作判定源，
    /// 该源本身不外发（`serde(skip)`）。只带类型词（"身份证号"），绝不带命中的内容片段。
    #[serde(skip)]
    pub sensitive: Option<&'static str>,
    /// 列表查询现算的两个读时旗标（02 §2.1/#3-P0）：让 `get_fragments` 摘要与
    /// `to_summary(build_detail)`（检索/相关路）口径一致——整理模式与卡片在摘要态也能看到
    /// 「已修订」「低价值」徽标。判定与 `build_detail` 同序（隐私优先于垃圾、stale 独立附加）。
    #[serde(skip)]
    pub stale: bool,
    #[serde(skip)]
    pub junk: bool,
}

/// 列表筛选（02 §2.1 query.layer/status/category/tag/source）。
#[derive(Debug, Default, Clone, Copy)]
pub struct ListFilter<'a> {
    pub layer: Option<&'a str>,
    pub status: Option<&'a str>,
    pub category: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub source: Option<&'a str>,
    pub media_type: Option<&'a str>,
    pub reviewed: Option<bool>,
    pub archived_by: Option<&'a str>,
}

/// 构造 WHERE 条件与位置参数（不含 LIMIT/OFFSET，由调用方追加）。
fn build_where(filter: &ListFilter) -> (Vec<String>, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;
    let mut conds = vec![
        "NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)".to_string(),
    ];
    let mut args: Vec<Value> = vec![];
    let mut add = |col: &str, v: &str| {
        conds.push(format!("{col} ?{}", args.len() + 1));
        args.push(Value::Text(v.to_string()));
    };
    if let Some(s) = filter.status {
        add("s.status =", s);
    }
    if let Some(l) = filter.layer {
        add("f.layer =", l);
    }
    if let Some(m) = filter.media_type {
        add("f.media_type =", m);
    }
    if let Some(c) = filter.category {
        add("r.category =", c);
    }
    if let Some(s) = filter.source {
        add("f.source =", s);
    }
    if let Some(ab) = filter.archived_by {
        add("f.archived_by =", ab);
    }
    if let Some(rv) = filter.reviewed {
        // reviewed 落库是 INTEGER 0/1（03 §3.9），位置参数须同型
        conds.push(format!("f.reviewed = ?{}", args.len() + 1));
        args.push(Value::Integer(rv as i64));
    }
    if let Some(t) = filter.tag {
        // tags 为 JSON 数组文本，03 §3.3 约定用 LIKE '%"xx"%' 查询
        conds.push(format!("r.tags LIKE ?{}", args.len() + 1));
        args.push(Value::Text(format!("%\"%{t}\"%")));
    }
    (conds, args)
}

/// 列表查询（03 §5 ④），排除墓碑。offset/limit：limit 默认 20 上限 100（02 通用约定）。
pub fn list(
    conn: &Connection,
    offset: i64,
    limit: i64,
    filter: &ListFilter,
) -> Result<Vec<FragmentListItem>, AppError> {
    use rusqlite::types::Value;
    let limit = limit.clamp(1, 100);
    let (conds, mut args) = build_where(filter);
    args.push(Value::Integer(limit));
    args.push(Value::Integer(offset));
    let n = args.len();
    let sql = format!(
        "SELECT f.id, f.title, substr(f.content,1,120), s.status, r.category, r.tags, f.created_at, \
                f.layer, f.media_type, f.archived_by, substr(f.content,1,4000), \
                f.content_updated_at, r.processed_at, f.media_path
           FROM fragments f
           JOIN fragment_status s ON s.fragment_id = f.id
           LEFT JOIN processing_results r
                  ON r.fragment_id = f.id AND r.superseded = 0
          WHERE {}
          ORDER BY f.created_at DESC, f.id DESC
          LIMIT ?{} OFFSET ?{}",
        conds.join(" AND "),
        n - 1,
        n,
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            let tags_json: Option<String> = r.get(5)?; // LEFT JOIN 无结果为 NULL
            let tags: Vec<String> = tags_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default();
            let detect_src: String = r.get(10)?;
            let status: String = r.get(3)?;
            let media_type: String = r.get(8)?;
            let sensitive =
                crate::db::triage::detect_sensitive(&detect_src).map(crate::db::triage::sensitive_label);
            // 「已修订」：正文改在结果生成之后（读时算，与 build_detail 同判据；仅有结果时才算过期）。
            let content_updated_at: Option<String> = r.get(11)?;
            let latest_processed_at: Option<String> = r.get(12)?;
            let stale = content_updated_at
                .as_deref()
                .zip(latest_processed_at.as_deref())
                .map(|(cua, proc)| cua > proc)
                .unwrap_or(false);
            // 「低价值」：仅 skipped 且未命中隐私时判定（隐私优先，与 build_detail 同序）。
            // 图片排除在外（批次21-B）：它的 skipped 是"设计上不经模型"（02 §1.3），不是闸门判低——
            // 那行占位正文只有一个文件名，`assess` 必然判短，挂上「低价值」徽标就是撒谎。
            let junk = status == "skipped"
                && media_type != "image"
                && sensitive.is_none()
                && crate::db::triage::assess(&detect_src)
                    .map(|reason| reason.is_junk())
                    .unwrap_or(false);
            Ok(FragmentListItem {
                id: r.get(0)?,
                title: r.get(1)?,
                excerpt: r.get(2)?,
                status,
                category: r.get(4)?,
                tags,
                created_at: r.get(6)?,
                layer: r.get(7)?,
                media_type,
                archived_by: r.get(9)?,
                media_path: r.get(13)?,
                sensitive,
                stale,
                junk,
            })
        })
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 活跃片段数（同筛选条件），供分页 total。
pub fn count_active(conn: &Connection, filter: &ListFilter) -> Result<i64, AppError> {
    let (conds, args) = build_where(filter);
    let sql = format!(
        "SELECT count(*) FROM fragments f
           JOIN fragment_status s ON s.fragment_id = f.id
           LEFT JOIN processing_results r ON r.fragment_id = f.id AND r.superseded = 0
          WHERE {}",
        conds.join(" AND ")
    );
    conn.query_row(&sql, rusqlite::params_from_iter(args.iter()), |r| r.get(0))
        .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 同筛选条件下的**全部** id（不分页）。导出整库/整个筛选结果要用：`list` 的 limit 上限 100
/// 是分页口径，不该变成导出的隐性截断。JOIN 结构与 `list` 保持一致，使"导出的范围"=="看到的范围"。
pub fn ids_matching(conn: &Connection, filter: &ListFilter) -> Result<Vec<String>, AppError> {
    let (conds, args) = build_where(filter);
    let sql = format!(
        "SELECT f.id FROM fragments f
           JOIN fragment_status s ON s.fragment_id = f.id
           LEFT JOIN processing_results r ON r.fragment_id = f.id AND r.superseded = 0
          WHERE {}
          ORDER BY f.created_at DESC, f.id DESC",
        conds.join(" AND ")
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| r.get::<_, String>(0))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| AppError::DbRead(e.to_string()))
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Fragment>, AppError> {
    conn.query_row(
        &format!(
            "SELECT {FRAGMENT_COLS} FROM fragments f
              WHERE f.id = ?1
                AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)"
        ),
        params![id],
        row_to_fragment,
    )
    .optional()
    .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 按 content_hash 找活跃片段（剪贴板去重入口，02 §1.2）。
/// "活跃"谓词与唯一索引 `uq_frags_active_hash`（v2：`layer <> 'trash'`）严格对齐——
/// 垃圾站里的行不占去重槽（丢过同一段再收是合法的重收集），故必须排除 trash。
pub fn find_active_by_hash(conn: &Connection, hash: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT f.id FROM fragments f
          WHERE f.content_hash = ?1
            AND f.layer <> 'trash'
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)",
        params![hash],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 按原文查活跃片段 id（手工收集的幂等入口，02 §1.1）。
pub fn active_id_by_text(conn: &Connection, content: &str) -> Result<Option<String>, AppError> {
    find_active_by_hash(conn, &hex_sha256(content))
}

pub struct NewFragment<'a> {
    pub content: &'a str,
    pub title: Option<&'a str>,
    pub source: &'a str,
    pub external_url: Option<&'a str>,
    /// 'text'(默认) | 'link' | 'image'；None 走 'text'。
    pub media_type: Option<&'a str>,
    /// 人工附言（02 §1.1），并入 FTS content 参与检索。
    pub note: Option<&'a str>,
    /// 批次21-B（02 §1.3）：图片文件名（相对 `media` 目录，见 03 §3.1 `media_path`）。非图片为 None。
    pub media_path: Option<&'a str>,
    /// 指定主键（None=本函数生成 UUID v4）。只有图片路径需要预先指定：文件名契约是
    /// `<fragment_id>.<ext>`，而正文占位行又要含该文件名才躲得开活跃去重唯一索引，
    /// 两者都必须在 insert 之前知道 id。其余录入一律 None。
    pub id: Option<&'a str>,
}

/// 校验 + 写 fragments + fragment_status(pending) + FTS 行（02 §1.1 行为）。
/// 入参校验是写路径边界：重复 hash 由唯一索引拦截，调用方先用 find_active_by_hash 判断。
pub fn insert(conn: &Connection, input: &NewFragment) -> Result<Fragment, AppError> {
    if input.content.trim().is_empty() {
        return Err(AppError::InputEmpty);
    }
    let char_count = input.content.chars().count() as i64;
    if char_count as usize > MAX_CHARS {
        return Err(AppError::InputTooLarge);
    }
    if !matches!(input.source, "manual" | "clipboard" | "link") {
        return Err(AppError::InputInvalid(format!("source: {}", input.source)));
    }
    let media_type = input.media_type.unwrap_or("text");
    if !matches!(media_type, "text" | "link" | "image") {
        return Err(AppError::InputInvalid(format!("media_type: {media_type}")));
    }
    let id = input.id.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let hash = hex_sha256(input.content);
    let now = now_iso();
    // 主表+队列+FTS 三步原子：崩溃不留"无队列片段/无 FTS 行"的孤儿
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO fragments
           (id, content, title, source, content_hash, external_url, char_count, media_type, note, media_path, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)",
        params![id, input.content, input.title, input.source, hash, input.external_url, char_count, media_type, input.note, input.media_path, &now],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(ref f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
            AppError::StateConflict
        }
        other => AppError::DbWrite(other.to_string()),
    })?;
    jobs::enqueue(&tx, &id, &now)?;
    let rowid = rowid_of(&tx, &id)?;
    fts_upsert(&tx, rowid, input.title, &indexed_text(input.content, input.note), "[]")?;
    tx.commit()?;
    get(conn, &id)?.ok_or(AppError::Internal("insert 后读取失败".into()))
}

/// FTS 索引正文 = content +（有附言时）note，使人工附言可被关键词命中（02 §1.1）。
fn indexed_text(content: &str, note: Option<&str>) -> String {
    match note.filter(|n| !n.trim().is_empty()) {
        Some(n) => format!("{content}\n{n}"),
        None => content.to_string(),
    }
}

/// 入队前纯启发式判定（02 §1.1、03 §2 skipped）：用户手填标题=明确要整理，放行；
/// 否则交给 `triage::assess`。返回 `Some(reason)` 表示应跳过 AI 处理。
/// **隐私不受标题豁免**：填标题是"要求整理"，不是"授权把身份证发给模型"——
/// 前者可以撤回（重新处理按钮），后者不可撤回，故隐私判定优先且强制。
pub fn assess_skip(content: &str, has_manual_title: bool) -> Option<triage::SkipReason> {
    if triage::detect_sensitive(content).is_some() {
        return Some(triage::SkipReason::Sensitive);
    }
    if has_manual_title {
        return None;
    }
    triage::assess(content)
}

/// 录入但闸门判定无需处理：走标准 `insert`（主表+FTS），随即置 `skipped` 令 worker 永不领取。
/// 落库后该行不再是 pending，领取查询（WHERE status='pending'）不会命中，零 token 消耗。
pub fn insert_skipped(conn: &Connection, input: &NewFragment) -> Result<Fragment, AppError> {
    let f = insert(conn, input)?;
    jobs::mark_skipped(conn, &f.id)?;
    get(conn, &f.id)?.ok_or(AppError::Internal("insert_skipped 后读取失败".into()))
}

pub(crate) fn rowid_of(conn: &Connection, id: &str) -> Result<i64, AppError> {
    conn.query_row("SELECT rowid FROM fragments WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(|e| AppError::DbRead(e.to_string()))?
        .ok_or(AppError::NotFound)
}

/// 覆盖写一行 FTS。自存储 fts5 表用普通 DML 维护索引
/// （'delete'/'update' 特殊命令仅适用于 external-content/contentless 表）。
pub(crate) fn fts_upsert(
    conn: &Connection,
    rowid: i64,
    title: Option<&str>,
    content: &str,
    tags_json: &str,
) -> Result<(), AppError> {
    conn.execute("DELETE FROM fts_fragments WHERE rowid = ?1", params![rowid])?;
    conn.execute(
        "INSERT INTO fts_fragments(rowid, title, content, tags) VALUES (?1,?2,?3,?4)",
        params![rowid, title, content, tags_json],
    )?;
    Ok(())
}

/// 结果写入后刷新 FTS 的 tags 列（03 §3.8 应用层显式 update 路径）。
pub fn fts_sync_tags(conn: &Connection, id: &str) -> Result<(), AppError> {
    let frag = get(conn, id)?.ok_or(AppError::NotFound)?;
    let rowid = rowid_of(conn, id)?;
    let tags_json: String = conn
        .query_row(
            "SELECT tags FROM processing_results
              WHERE fragment_id = ?1 AND superseded = 0
              ORDER BY version DESC LIMIT 1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::DbRead(e.to_string()))?
        .unwrap_or_else(|| "[]".into());
    fts_upsert(conn, rowid, frag.title.as_deref(), &indexed_text(&frag.content, frag.note.as_deref()), &tags_json)
}

/// 人工改标题：manual 优先，retry 不覆盖（02 §2.5）。同步 FTS。
pub fn set_title_manual(conn: &Connection, id: &str, title: Option<&str>) -> Result<(), AppError> {
    let n = conn.execute(
        "UPDATE fragments SET title = ?2, title_manual = 1, updated_at = ?3 WHERE id = ?1",
        params![id, title, now_iso()],
    )?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    fts_sync_tags(conn, id)
}

/// 人工改附言（02 §2.2 note）：空串视为清除（落 NULL）。note 进 FTS 索引词，改后同步刷新。
/// 顺带把这次改动记进 `edit_log`（02 §2.5 批次6-①）——此前笔记改了就没了，无从回看。
/// 无变化直接返回：点开textarea又失焦不该被记成一次修订。
pub fn set_note(conn: &Connection, id: &str, note: Option<&str>) -> Result<(), AppError> {
    let f = get(conn, id)?.ok_or(AppError::NotFound)?;
    let trimmed = note.map(str::trim).filter(|s| !s.is_empty());
    if trimmed == f.note.as_deref() {
        return Ok(());
    }
    let now = now_iso();
    let entry = EditEntry {
        at: now.clone(),
        field: "note".into(),
        before_chars: f.note.as_deref().unwrap_or("").chars().count() as i64,
        after_chars: trimmed.unwrap_or("").chars().count() as i64,
        excerpt: f.note.as_deref().map(edit_excerpt),
    };
    let log = edit_log_json(&push_edit(&f.edit_log, entry));
    conn.execute(
        "UPDATE fragments SET note = ?2, updated_at = ?3, edit_log = ?4 WHERE id = ?1",
        params![id, trimmed, now, log],
    )?;
    fts_sync_tags(conn, id)
}

/// 人工修订正文（批次6-①，02 §2.5）：**纯本地 UPDATE，绝不外发**（不调模型、不调 embedding）。
/// 一次改正文同时：重算 `char_count`/`content_hash`、刷 FTS、置 `content_updated_at`
/// （「结果已过期」读时判据的分子）、追加一条 `edit_log`。
/// 撞既有活跃同文 → `E_STATE_CONFLICT`，**不静默改 hash 绕过唯一索引**。
/// 向量不在这里删（保持纯 SQLite 可单测）——由 command 层编辑成功后尽力删旧向量。
pub fn set_content(conn: &Connection, id: &str, content: &str) -> Result<Fragment, AppError> {
    if content.trim().is_empty() {
        return Err(AppError::InputEmpty);
    }
    let char_count = content.chars().count() as i64;
    if char_count as usize > MAX_CHARS {
        return Err(AppError::InputTooLarge);
    }
    let f = get(conn, id)?.ok_or(AppError::NotFound)?;
    if content == f.content {
        return Ok(f); // 改回原样：无操作，不记修订也不推进过期时刻
    }
    let hash = hex_sha256(content);
    if let Some(other) = find_active_by_hash(conn, &hash)? {
        if other != id {
            return Err(AppError::StateConflict); // 与另一条活跃片段一模一样（去重索引会撞）
        }
    }
    let now = now_iso();
    let entry = EditEntry {
        at: now.clone(),
        field: "content".into(),
        before_chars: f.char_count,
        after_chars: char_count,
        excerpt: Some(edit_excerpt(&f.content)),
    };
    let log = edit_log_json(&push_edit(&f.edit_log, entry));
    conn.execute(
        "UPDATE fragments SET content=?2, char_count=?3, content_hash=?4, \
                content_updated_at=?5, updated_at=?5, edit_log=?6 WHERE id=?1",
        params![id, content, char_count, hash, now, log],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(ref f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            AppError::StateConflict
        }
        other => AppError::DbWrite(other.to_string()),
    })?;
    // 隐私闸门的时效：改过正文就作废此前那次"同意发送"的授权（授权买的是当时那份文本）。
    // 只在真正改成的这条路径上撤——上面 `content == f.content` 的无操作分支不该动状态。
    crate::db::jobs::clear_outbound(conn, id)?;
    fts_sync_tags(conn, id)?;
    get(conn, id)?.ok_or(AppError::Internal("set_content 后读取失败".into()))
}

/// 三层分拣迁移（02 §2.6）：buffer↔archived↔trash。
/// archived 主动放行置 archived_by='manual'+reviewed=1；trash 记 trashed_at；
/// 回 buffer 清除 trashed_at/archived_by 并复位 reviewed。返回迁移后的片段。
pub fn set_fragment_layer(conn: &Connection, id: &str, to: &str) -> Result<Fragment, AppError> {
    if get(conn, id)?.is_none() {
        return Err(AppError::NotFound);
    }
    if !matches!(to, "buffer" | "archived" | "trash") {
        return Err(AppError::InputInvalid(format!("layer: {to}")));
    }
    let now = now_iso();
    let (layer, archived_by, reviewed, trashed_at): (&str, Option<&str>, i64, Option<&str>) =
        match to {
            "archived" => ("archived", Some("manual"), 1, None),
            "trash" => ("trash", None, 0, Some(now.as_str())),
            _ => ("buffer", None, 0, None),
        };
    conn.execute(
        "UPDATE fragments SET layer=?2, archived_by=?3, reviewed=?4, trashed_at=?5, updated_at=?6 WHERE id=?1",
        params![id, layer, archived_by, reviewed, trashed_at, &now],
    )
    .map_err(|e| AppError::DbWrite(e.to_string()))?;
    get(conn, id)?.ok_or(AppError::NotFound)
}

/// 超时自动归档阈值（天，02 §2「超时归档（非命令）」固定值）。
/// 不做成配置项：改阈值等于改产品语义（多久没管就算"替你收"），属契约变更而非用户偏好。
pub const AUTO_ARCHIVE_DAYS: i64 = 14;

/// 超时归档谓词（`f` 为 fragments 别名，SELECT 与 UPDATE 共用一份，防两处判定漂移）。
/// 时间戳与 `now_iso` 同格式，故字典序比较即时间序（同 `week_activity`）。
/// 契约的 `created_at` 之外再加一道 `updated_at`：只按创建时间判超时，用户把条目「移回缓冲区」后
/// 下一轮扫描会立刻再收走，等于静默回滚用户的动作——正撞在「不打断用户」的反面。加了它，回退即再获窗口。
const EXPIRED_WHERE: &str = "f.layer = 'buffer' AND f.reviewed = 0 AND f.created_at < ?1 AND f.updated_at < ?1 \
      AND EXISTS (SELECT 1 FROM fragment_status s WHERE s.fragment_id = f.id AND s.status = 'done') \
      AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)";

/// 超时自动归档（02 §2 非命令）：处理完成却在缓冲区躺过 14 天的条目「替你收」进归档层。
/// 只改 `layer`/`archived_by`/`updated_at`，**`reviewed` 保持 0** → 归档库仍按未审呈现，可补审；
/// 用户在详情页「移回缓冲区」即可回退，且回退本身即续期（见 `EXPIRED_WHERE`）。
/// 不含 `skipped`：闸门判低的条目该由用户裁决丢弃与否，替他收进归档是把垃圾制度化。
/// 返回本次归档的 id（调用方逐个发 `fragment://updated`）；幂等，重复调用返回空。
pub fn auto_archive_expired(conn: &Connection, cutoff: &str) -> Result<Vec<String>, AppError> {
    let select = format!("SELECT f.id FROM fragments f WHERE {EXPIRED_WHERE} ORDER BY f.created_at");
    let mut stmt = conn
        .prepare(&select)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let ids: Vec<String> = stmt
        .query_map(params![cutoff], |r| r.get(0))
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<String>>>())
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    if ids.is_empty() {
        return Ok(ids);
    }
    let update = format!(
        "UPDATE fragments SET layer='archived', archived_by='auto', updated_at=?2 \
          WHERE id IN (SELECT f.id FROM fragments f WHERE {EXPIRED_WHERE})"
    );
    // 单语句天然原子，无需事务；UPDATE 复用同一谓词，并发的人工放行不会被覆盖。
    conn.execute(&update, params![cutoff, now_iso()])
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
    Ok(ids)
}

/// 回收站滞留阈值（天，02 §2.1「30 天后由后台 `purge` 硬删」）。
/// 与 `AUTO_ARCHIVE_DAYS` 同理不做配置项：改阈值=改"你的数据多久真的没了"的产品语义。
pub const PURGE_AFTER_DAYS: i64 = 30;

/// 物理删除（02 §2.8）：无痕清掉主表行 + FTS + 墓碑，其余关联行由 `ON DELETE CASCADE` 收走
/// （fragment_status / processing_results / fragment_links…）。
/// 墓碑一并删是刻意的：留着它，同原文日后重新收集会永远被判「你此前丢弃过」，
/// 而"彻底删除"的用户意图就是无痕。
/// 向量行不在此处清理——本函数保持纯 SQLite 可单测，虚表由调用方经 `VectorStore::delete` 负责。
pub fn purge(conn: &Connection, id: &str) -> Result<(), AppError> {
    let rowid = rowid_of(conn, id)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM fts_fragments WHERE rowid = ?1", params![rowid])?;
    tx.execute("DELETE FROM deletions WHERE fragment_id = ?1", params![id])?;
    tx.execute("DELETE FROM fragments WHERE id = ?1", params![id])?;
    tx.commit().map_err(|e| AppError::DbWrite(e.to_string()))?;
    Ok(())
}

/// 到期清除（02 §2「非命令」）：在回收站躺过 30 天的条目物理删除，令倒计时成为事实。
/// 逐条提交：一条失败（例如正被占用）不该带走整批；谓词幂等，下次扫描重来。
/// 返回 `(id, media_path)`——SQL 删完行就再也问不出文件名了，而批次21-B 要求"彻底删除"
/// 连图片文件一起清掉，故必须在同一条删除路径上把文件名交给调用方（不另开一次扫描，
/// 免得两处到期谓词漂移）。向量与文件由 services 层清理。
pub fn purge_expired(conn: &Connection, cutoff: &str) -> Result<Vec<(String, Option<String>)>, AppError> {
    let mut stmt = conn
        .prepare("SELECT id, media_path FROM fragments WHERE layer='trash' AND trashed_at IS NOT NULL AND trashed_at < ?1")
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows: Vec<(String, Option<String>)> = stmt
        .query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?)))
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<(String, Option<String>)>>>())
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    drop(stmt);
    let mut out = Vec::with_capacity(rows.len());
    for (id, media) in rows {
        purge(conn, &id)?;
        out.push((id, media));
    }
    Ok(out)
}

/// 软删除：写墓碑 + 清 FTS 行 + 释放 hash 位；主表行与结果保留，
/// 硬删除由 `purge`（命令）与 `purge_expired`（30 天到期扫描）做（02 §2.4、§2.8）。
/// 释放 hash 是"活跃片段内去重"（03 §3.1 部分唯一索引）成立的必要条件：
/// 墓碑行若保留原 hash，同内容将永久无法再收集。
pub fn soft_delete(conn: &Connection, id: &str) -> Result<(), AppError> {
    if get(conn, id)?.is_none() {
        return Err(AppError::NotFound);
    }
    let rowid = rowid_of(conn, id)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO deletions (fragment_id, deleted_at) VALUES (?1, ?2)",
        params![id, now_iso()],
    )?;
    tx.execute("DELETE FROM fts_fragments WHERE rowid = ?1", params![rowid])?;
    tx.execute(
        "UPDATE fragments SET content_hash = content_hash || '-del#' || id WHERE id = ?1",
        params![id],
    )?;
    tx.commit()?;
    Ok(())
}

/// 关键词检索（03 §5 ③，bm25 越小越好，转成正分数倒序）。
/// 用户输入整体作短语匹配（引号转义为双写），避免 FTS5 语法字符（* / NEAR / 裸引号）报错或改变语义。
/// `exclude_layers` 与 `live_id_set` 同一口径，在 SQL 里裁剪后再 `LIMIT`——先取满 top-N 再丢弃会让结果凭空变短。
pub fn fts_search(
    conn: &Connection,
    query: &str,
    limit: i64,
    exclude_layers: &[&str],
) -> Result<Vec<(String, f64)>, AppError> {
    if query.trim().is_empty() {
        return Err(AppError::InputEmpty);
    }
    let layer_filter = if exclude_layers.is_empty() {
        String::new()
    } else {
        format!(" AND f.layer NOT IN ({})", vec!["?"; exclude_layers.len()].join(","))
    };
    let sql = format!(
        "SELECT f.id, bm25(fts_fragments) AS rank
           FROM fts_fragments
           JOIN fragments f ON f.rowid = fts_fragments.rowid
          WHERE fts_fragments MATCH ?1
            {layer_filter}
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)
          ORDER BY rank
          LIMIT ?{}",
        exclude_layers.len() + 2,
    );
    let phrase = format!("\"{}\"", query.trim().replace('"', "\"\""));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    // 顺序与占位符一致：MATCH → 各 exclude_layer → LIMIT
    let mut bound: Vec<&dyn rusqlite::types::ToSql> = vec![&phrase];
    bound.extend(exclude_layers.iter().map(|s| s as &dyn rusqlite::types::ToSql));
    bound.push(&limit);
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bound.iter()), |r| {
            // bm25 负值→ (0,∞) 分数，越大越相关
            let rank: f64 = r.get(1)?;
            Ok((r.get::<_, String>(0)?, -rank))
        })
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 向量候选的可见性过滤：向量虚表只认 id，不知道 layer/软删，故与 `fts_search` 用同一规则
/// （排除墓碑，按需排除指定层，02 §7.4「排除自身与 trash」）。返回仍可见的 id 集合。
/// 入参只贡献占位符个数，值一律走绑定，不拼进 SQL。
pub fn live_id_set(
    conn: &Connection,
    ids: &[String],
    exclude_layers: &[&str],
) -> Result<std::collections::HashSet<String>, AppError> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let layer_filter = if exclude_layers.is_empty() {
        String::new()
    } else {
        format!(" AND f.layer NOT IN ({})", vec!["?"; exclude_layers.len()].join(","))
    };
    let sql = format!(
        "SELECT f.id FROM fragments f
          WHERE f.id IN ({}){}
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)",
        vec!["?"; ids.len()].join(","),
        layer_filter
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let mut bound: Vec<String> = ids.to_vec();
    bound.extend(exclude_layers.iter().map(|s| s.to_string()));
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bound.iter()), |r| r.get::<_, String>(0))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let mut out = std::collections::HashSet::new();
    for row in rows {
        out.insert(row.map_err(|e| AppError::DbRead(e.to_string()))?);
    }
    Ok(out)
}

/// 字符二元组集合（忽略空白）。中文按字、拉丁按字母，够用于"同一段东西"级别的近似判定。
fn bigram_set(s: &str) -> std::collections::HashSet<(char, char)> {
    let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Dice 系数 = 2|A∩B| / (|A|+|B|)，两侧皆空视为 0（不把"双双无二元组"判成完全相同）。
fn dice(a: &std::collections::HashSet<(char, char)>, b: &std::collections::HashSet<(char, char)>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.iter().filter(|g| b.contains(*g)).count();
    2.0 * inter as f64 / (a.len() + b.len()) as f64
}

/// 相关碎片候选（02 §7.4）：排除自身与垃圾站，阈值 0.15，按分数降序取前 k。
/// 向量近邻未接进 command 层前用 bigram Dice（契约明示的兜底算法）；扫描上限 500 条近期片段，
/// 避免整库两两比较随规模线性恶化（个人收藏量级下足够，超出即"看不见的近邻"而非卡顿）。
pub fn related_by_dice(conn: &Connection, fragment_id: &str, k: usize) -> Result<Vec<(String, f64)>, AppError> {
    let target = match get(conn, fragment_id)? {
        Some(f) => f,
        None => return Ok(vec![]),
    };
    let a = bigram_set(&target.content);
    if a.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn
        .prepare(
            "SELECT f.id, f.content FROM fragments f
              WHERE f.id <> ?1 AND f.layer <> 'trash'
                AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)
              ORDER BY f.created_at DESC LIMIT 500",
        )
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map(params![fragment_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let mut scored: Vec<(String, f64)> = rows
        .filter_map(|row| row.ok())
        .map(|(id, content)| {
            let score = dice(&a, &bigram_set(&content));
            (id, score)
        })
        .filter(|(_, score)| *score >= 0.15)
        .collect();
    scored.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    Ok(scored)
}

/// 近 7 天分拣计量（02 §7.4 `get_week_digest` 的数据面）。
pub struct WeekActivity {
    /// 本周人工放行归档
    pub sorted: i64,
    /// 本周丢弃
    pub trashed: i64,
    /// 本周超时自动归档
    pub auto_archived: i64,
    /// 当前缓冲区待分拣总数（不限本周）
    pub pending: i64,
    /// 本周归档里出现最多的分类及其条数
    pub top_category: Option<(String, i64)>,
}

/// 周计量：时间戳一律 `now_iso` 同格式，故字典序比较即时间序（无需日期函数）。
pub fn week_activity(conn: &Connection) -> Result<WeekActivity, AppError> {
    let since = crate::db::iso_days_ago(7);
    let count = |sql: &str| -> Result<i64, AppError> {
        conn.query_row(sql, params![since], |r| r.get(0))
            .map_err(|e| AppError::DbRead(e.to_string()))
    };
    let sorted = count(
        "SELECT count(*) FROM fragments f
          WHERE f.layer = 'archived' AND f.archived_by = 'manual' AND f.updated_at >= ?1
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)",
    )?;
    let auto_archived = count(
        "SELECT count(*) FROM fragments f
          WHERE f.layer = 'archived' AND f.archived_by = 'auto' AND f.updated_at >= ?1
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)",
    )?;
    let trashed = count(
        "SELECT count(*) FROM fragments f
          WHERE f.layer = 'trash' AND COALESCE(f.trashed_at, f.updated_at) >= ?1
            AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)",
    )?;
    // 「待分拣」口径与首页 triageCount 严格一致（批次25 计数诚实化）：缓冲区里**未审**、且已 `done`
    // 或被闸门 `skipped`（图片原样收藏/低价值/含隐私）——两者都等用户拍板放行或丢弃。
    // 在途（pending/running）、失败（只能重试或丢弃，不算分拣）、已审（reviewed=1）都不计入。
    // 旧实现是 `count_active(layer='buffer')`＝整层活跃条目，与本字段注释「待分拣总数」自相矛盾，
    // 且让 LIVE 下顶部「待分拣 N 条」与周回顾「待分拣还有 M 条」两个数打脸。
    let pending = conn
        .query_row(
            "SELECT count(*) FROM fragments f
               JOIN fragment_status s ON s.fragment_id = f.id
              WHERE f.layer = 'buffer' AND f.reviewed = 0
                AND s.status IN ('done', 'skipped')
                AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let top_category: Option<(String, i64)> = conn
        .query_row(
            "SELECT r.category, count(*) c FROM fragments f
               JOIN fragment_status s ON s.fragment_id = f.id
               JOIN processing_results r ON r.fragment_id = f.id AND r.superseded = 0
              WHERE f.layer = 'archived' AND f.updated_at >= ?1
                AND NOT EXISTS (SELECT 1 FROM deletions d WHERE d.fragment_id = f.id)
              GROUP BY r.category ORDER BY c DESC, r.category LIMIT 1",
            params![since],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    Ok(WeekActivity { sorted, trashed, auto_archived, pending, top_category })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nf(content: &str) -> NewFragment<'_> {
        NewFragment {
            content,
            title: None,
            source: "manual",
            external_url: None,
            media_type: None,
            note: None,
            media_path: None,
            id: None,
        }
    }

    /// 建一条走完队列（`done`）的片段，并把 `created_at`/`updated_at` 一起拨老到 days 天前
    /// （即"收集后无人再碰过它"，超时归档测试用）。`insert` 已入队，故逐条 claim→done，
    /// 避免 `claim_next` 一次领走多条而牵连其它在测片段。
    fn add_done_aged(conn: &Connection, content: &str, days: i64) -> String {
        let f = insert(conn, &nf(content)).unwrap();
        jobs::claim_next(conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();
        jobs::mark_done(conn, &f.id).unwrap();
        backdate(conn, &f.id, days);
        assert_eq!(jobs::status_of(conn, &f.id).unwrap(), Some(jobs::Status::Done));
        f.id
    }

    fn backdate(conn: &Connection, id: &str, days: i64) {
        let then = crate::db::iso_days_ago(days);
        conn.execute(
            "UPDATE fragments SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![id, &then],
        )
        .unwrap();
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let conn = crate::db::test_conn();
        let f = insert(&conn, &nf("SQLite FTS5 适合本地全文检索")).unwrap();
        let got = get(&conn, &f.id).unwrap().unwrap();
        assert_eq!(got, f);
        assert_eq!(got.source, "manual");
        assert_eq!(got.char_count, "SQLite FTS5 适合本地全文检索".chars().count() as i64);
        assert_eq!(got.content_hash, hex_sha256(&got.content));
        // 入队 pending
        let status: String = conn
            .query_row("SELECT status FROM fragment_status WHERE fragment_id = ?1", params![&f.id], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "pending");
    }

    #[test]
    fn rejects_empty_and_oversize_and_bad_source() {
        let conn = crate::db::test_conn();
        assert!(matches!(insert(&conn, &nf("   ")), Err(AppError::InputEmpty)));
        let big: String = "字".repeat(MAX_CHARS + 1);
        assert!(matches!(insert(&conn, &nf(&big)), Err(AppError::InputTooLarge)));
        let r = insert(&conn, &NewFragment { content: "x", title: None, source: "wechat", external_url: None, media_type: None, note: None, media_path: None, id: None });
        assert!(matches!(r, Err(AppError::InputInvalid(_))));
    }

    #[test]
    fn duplicate_hash_is_state_conflict() {
        let conn = crate::db::test_conn();
        insert(&conn, &nf("同一段内容")).unwrap();
        let dup = insert(&conn, &nf("同一段内容"));
        assert!(matches!(dup, Err(AppError::StateConflict)));
        assert_eq!(count_active(&conn, &ListFilter::default()).unwrap(), 1);
    }

    #[test]
    fn hash_lookup_and_list_and_pagination() {
        let conn = crate::db::test_conn();
        let a = insert(&conn, &nf("第一篇内容 Rust 所有权")).unwrap();
        assert_eq!(find_active_by_hash(&conn, &a.content_hash).unwrap().as_deref(), Some(a.id.as_str()));
        for i in 0..25 {
            let text = format!("第{i}篇内容，用于分页测试");
            insert(&conn, &nf(&text)).unwrap();
        }
        let none = ListFilter::default();
        assert_eq!(count_active(&conn, &none).unwrap(), 26);
        let page1 = list(&conn, 0, 20, &none).unwrap();
        let page2 = list(&conn, 20, 20, &none).unwrap();
        assert_eq!(page1.len(), 20);
        assert_eq!(page2.len(), 6);
        assert!(page1[0].created_at >= page1[19].created_at);
        assert!(page1.iter().all(|x| x.excerpt.chars().count() <= 120));
        // 筛选（02 §2.1）：status / source
        let done_only = list(&conn, 0, 20, &ListFilter { status: Some("done"), ..Default::default() }).unwrap();
        assert!(done_only.is_empty()); // 未处理
        let cnt_failed = count_active(&conn, &ListFilter { status: Some("pending"), ..Default::default() }).unwrap();
        assert_eq!(cnt_failed, 26);
        let clip = count_active(&conn, &ListFilter { source: Some("clipboard"), ..Default::default() }).unwrap();
        assert_eq!(clip, 0);
    }

    #[test]
    fn list_filter_by_category_and_tag() {
        let conn = crate::db::test_conn();
        let a = insert(&conn, &nf("分类筛选测试内容甲")).unwrap();
        let b = insert(&conn, &nf("分类筛选测试内容乙")).unwrap();
        crate::db::results::insert_result(&conn, &a.id, "技术", None, &["SQLite优化".to_string()], "", &[], false, None).unwrap();
        let f = ListFilter { category: Some("技术"), ..Default::default() };
        assert_eq!(count_active(&conn, &f).unwrap(), 1);
        assert_eq!(list(&conn, 0, 20, &f).unwrap()[0].id, a.id);
        let tagf = ListFilter { tag: Some("SQLite优化"), ..Default::default() };
        assert_eq!(list(&conn, 0, 20, &tagf).unwrap().len(), 1);
        let miss = ListFilter { tag: Some("不存在的标签"), ..Default::default() };
        assert_eq!(count_active(&conn, &miss).unwrap(), 0);
        let _ = b;
    }

    #[test]
    fn list_filter_by_reviewed_and_archived_by() {
        let conn = crate::db::test_conn();
        // 主动放行一条（archived_by=manual, reviewed=1）
        let manual = insert(&conn, &nf("人工放行的归档条目")).unwrap();
        set_fragment_layer(&conn, &manual.id, "archived").unwrap();
        // 超时自动归档一条（archived_by=auto, reviewed 保持 0）——补审队列的目标
        let auto = insert(&conn, &nf("超时替你收的归档条目")).unwrap();
        conn.execute(
            "UPDATE fragments SET layer='archived', archived_by='auto', reviewed=0 WHERE id=?1",
            rusqlite::params![&auto.id],
        )
        .unwrap();

        // 只看未补审的归档条目：命中 auto 那条
        let review_queue = ListFilter { layer: Some("archived"), reviewed: Some(false), ..Default::default() };
        let ids: Vec<String> = list(&conn, 0, 20, &review_queue).unwrap().into_iter().map(|x| x.id).collect();
        assert_eq!(ids, vec![auto.id.clone()]);
        // reviewed=true → 只剩主动放行的
        let reviewed = ListFilter { layer: Some("archived"), reviewed: Some(true), ..Default::default() };
        assert_eq!(count_active(&conn, &reviewed).unwrap(), 1);
        // 按 archived_by 精确取 auto
        let by_auto = ListFilter { archived_by: Some("auto"), ..Default::default() };
        assert_eq!(list(&conn, 0, 20, &by_auto).unwrap()[0].id, auto.id);
        // 两维组合 + 缺省不筛（None）不改变结果
        let none = ListFilter::default();
        assert_eq!(count_active(&conn, &none).unwrap(), 2);
    }

    #[test]
    fn soft_delete_hides_from_get_list_and_fts() {
        let conn = crate::db::test_conn();
        let f = insert(&conn, &nf("将被删除的片段内容")).unwrap();
        assert!(!fts_search(&conn, "将被删除", 10, &[]).unwrap().is_empty());
        soft_delete(&conn, &f.id).unwrap();
        assert!(get(&conn, &f.id).unwrap().is_none());
        assert!(find_active_by_hash(&conn, &f.content_hash).unwrap().is_none());
        assert_eq!(count_active(&conn, &ListFilter::default()).unwrap(), 0);
        assert!(fts_search(&conn, "将被删除", 10, &[]).unwrap().is_empty());
        // 主表行仍在（墓碑模式，硬删除由后台任务做）
        let n: i64 = conn
            .query_row("SELECT count(*) FROM fragments WHERE id = ?1", params![&f.id], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // 重复删除报 NotFound
        assert!(matches!(soft_delete(&conn, &f.id), Err(AppError::NotFound)));
        // 软删后同内容可再收集（hash 槽位已释放）
        let again = insert(&conn, &nf("将被删除的片段内容"));
        assert!(again.is_ok());
    }

    /// 计数工具：单测里断言"痕象全无"要覆盖三处——主表行、FTS 影子行、墓碑。
    /// FTS 虚表按 rowid 存，故先记下 rowid 再删。
    fn count_table(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn purge_leaves_no_row_no_fts_entry_no_tombstone() {
        let conn = crate::db::test_conn();
        let keep = insert(&conn, &nf("留着的那一条内容，用于对照计数")).unwrap();
        let gone = insert(&conn, &nf("将被彻底删除的片段内容")).unwrap();
        soft_delete(&conn, &gone.id).unwrap(); // 墓碑态下彻底删除：连同墓碑一起抹掉
        let keep_rowid = rowid_of(&conn, &keep.id).unwrap();
        let gone_rowid = rowid_of(&conn, &gone.id).unwrap();

        purge(&conn, &gone.id).unwrap();

        assert_eq!(get(&conn, &gone.id).unwrap(), None);
        assert_eq!(count_table(&conn, "fragments"), 1);
        assert_eq!(count_table(&conn, "deletions"), 0, "彻底删除须不留痕：墓碑一并清除");
        let orphan_status: i64 = conn
            .query_row(
                "SELECT count(*) FROM fragment_status WHERE fragment_id = ?1",
                params![&gone.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_status, 0, "CASCADE 关联行须随主行消失");
        let fts_rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM fts_fragments WHERE rowid IN (?1, ?2)",
                params![gone_rowid, keep_rowid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts_rows, 1, "只该剩下留着那条的 FTS 行");
        // 留着的那条完好
        assert!(get(&conn, &keep.id).unwrap().is_some());
        // 同原文日后可作为全新片段再收集，且不再被 retrash 判为"你丢弃过"
        let revived = insert(&conn, &nf("将被彻底删除的片段内容")).unwrap();
        assert_ne!(revived.id, gone.id);
        assert_eq!(count_table(&conn, "deletions"), 0);
        // 不存在的 id 报 NotFound
        assert!(matches!(purge(&conn, "no-such-id"), Err(AppError::NotFound)));
    }

    #[test]
    fn purge_expired_only_clears_aged_trash_and_is_idempotent() {
        let conn = crate::db::test_conn();
        let old = insert(&conn, &nf("回收站里躺了 31 天的一条")).unwrap().id;
        let fresh = insert(&conn, &nf("刚进回收站三天的一条")).unwrap().id;
        let buffer = insert(&conn, &nf("还在缓冲区三十一天的那条")).unwrap().id;
        for id in [&old, &fresh, &buffer] {
            conn.execute(
                "UPDATE fragments SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![id, crate::db::iso_days_ago(31)],
            )
            .unwrap();
        }
        for id in [&old, &fresh] {
            set_fragment_layer(&conn, id, "trash").unwrap();
        }
        // trashed_at 才是倒计时起点：老的拨到 31 天前，新的留 3 天前
        conn.execute("UPDATE fragments SET trashed_at = ?2 WHERE id = ?1", params![&old, crate::db::iso_days_ago(31)]).unwrap();
        conn.execute("UPDATE fragments SET trashed_at = ?2 WHERE id = ?1", params![&fresh, crate::db::iso_days_ago(3)]).unwrap();

        let cutoff = crate::db::iso_days_ago(PURGE_AFTER_DAYS);
        // 返回 (id, 图片文件名)：非图片那条是 None（批次21-B 要据此顺手删文件）。
        assert_eq!(purge_expired(&conn, &cutoff).unwrap(), vec![(old.clone(), None)]);
        assert!(get(&conn, &old).unwrap().is_none());
        assert!(get(&conn, &fresh).unwrap().is_some(), "未到期的回收站条目不得被动");
        assert_eq!(get(&conn, &buffer).unwrap().unwrap().layer, "buffer", "未丢弃的内容更不会被物理删除");
        // 幂等：同一批不会再清掉任何一条
        assert!(purge_expired(&conn, &cutoff).unwrap().is_empty());
    }

    #[test]
    fn fts_trigram_search_ranks_and_syncs_tags() {
        let conn = crate::db::test_conn();
        let f = insert(&conn, &NewFragment { content: "trigram tokenizer 中文检索测试", title: Some("中文标题"), source: "manual", external_url: None, media_type: None, note: None, media_path: None, id: None }).unwrap();
        let hits = fts_search(&conn, "中文检索", 10, &[]).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, f.id);
        assert!(hits[0].1 > 0.0);
        // 写入结果 tags 后，fts 的 tags 列可被检索命中
        crate::db::results::insert_result(
            &conn, &f.id, "技术", Some("数据库"), &["中文分词".to_string()], "", &[], false, None,
        )
        .unwrap();
        // trigram 分词器要求查询串 ≥3 字符
        let hits = fts_search(&conn, "中文分词", 10, &[]).unwrap();
        assert_eq!(hits[0].0, f.id);
        // 人工改标题后 FTS 跟进
        set_title_manual(&conn, &f.id, Some("新标题一二三")).unwrap();
        let hits = fts_search(&conn, "新标题一二三", 10, &[]).unwrap();
        assert_eq!(hits.len(), 1);
        // FTS5 语法字符不再报错（整体按短语处理）
        assert!(fts_search(&conn, "sqlite*\"NEAR(a b)", 10, &[]).is_ok());
    }

    #[test]
    fn insert_defaults_to_buffer_layer() {
        let conn = crate::db::test_conn();
        let f = insert(&conn, &nf("默认落在缓冲区的片段")).unwrap();
        assert_eq!(f.layer, "buffer");
        assert_eq!(f.media_type, "text");
        assert_eq!(f.reviewed, false);
        assert_eq!(f.archived_by, None);
    }

    #[test]
    fn set_fragment_layer_transitions() {
        let conn = crate::db::test_conn();
        let f = insert(&conn, &nf("将被分拣的片段内容")).unwrap();
        // 放行归档：archived_by=manual, reviewed=1
        let a = set_fragment_layer(&conn, &f.id, "archived").unwrap();
        assert_eq!(a.layer, "archived");
        assert_eq!(a.archived_by.as_deref(), Some("manual"));
        assert_eq!(a.reviewed, true);
        // 丢入垃圾站：记 trashed_at，清 archived_by
        let t = set_fragment_layer(&conn, &f.id, "trash").unwrap();
        assert_eq!(t.layer, "trash");
        assert!(t.trashed_at.is_some());
        assert_eq!(t.archived_by, None);
        // 回缓冲区：清 trashed_at / reviewed 复位
        let b = set_fragment_layer(&conn, &f.id, "buffer").unwrap();
        assert_eq!(b.layer, "buffer");
        assert!(b.trashed_at.is_none());
        assert_eq!(b.reviewed, false);
        // 非法目标 / 不存在
        assert!(matches!(set_fragment_layer(&conn, &f.id, "junk"), Err(AppError::InputInvalid(_))));
        assert!(matches!(set_fragment_layer(&conn, "nope", "trash"), Err(AppError::NotFound)));
    }

    #[test]
    fn auto_archive_expired_moves_old_done_to_archived() {
        let conn = crate::db::test_conn();
        let old = add_done_aged(&conn, "躺了 20 天没人分拣的已完成片段", 20);
        let young = add_done_aged(&conn, "才 3 天，仍在缓冲区等人", 3);
        let skipped = insert(&conn, &nf("✅✅😂")).unwrap().id; // 闸门命中形态：skipped，等人裁决
        jobs::mark_skipped(&conn, &skipped).unwrap();
        backdate(&conn, &skipped, 20);
        let pending = insert(&conn, &nf("收集 20 天但一直没排到处理")).unwrap().id;
        backdate(&conn, &pending, 20);

        let cutoff = crate::db::iso_days_ago(AUTO_ARCHIVE_DAYS);
        let gone = auto_archive_expired(&conn, &cutoff).unwrap();
        assert_eq!(gone, vec![old.clone()]);
        let a = get(&conn, &old).unwrap().unwrap();
        assert_eq!(a.layer, "archived");
        assert_eq!(a.archived_by.as_deref(), Some("auto"));
        assert!(!a.reviewed, "超时归档不代替用户审阅，仍可补审");
        for id in [&young, &skipped, &pending] {
            assert_eq!(get(&conn, id).unwrap().unwrap().layer, "buffer", "不该被替收");
        }
        // 幂等：同一批不会被收两次
        assert!(auto_archive_expired(&conn, &cutoff).unwrap().is_empty());
        // 周计量的 autoArchived 由此真实非零（此前恒 0）
        assert_eq!(week_activity(&conn).unwrap().auto_archived, 1);
    }

    #[test]
    fn auto_archive_expired_respects_user_touch_and_tombstone() {
        let conn = crate::db::test_conn();
        let a = add_done_aged(&conn, "很老的条目甲，准备替收", 20);
        let b = add_done_aged(&conn, "很老的条目乙，刚被编辑过", 20);
        let c = add_done_aged(&conn, "很老的条目丙，已被彻底删除", 20);
        set_note(&conn, &b, Some("刚补了附言")).unwrap(); // updated_at 刷新 → 再获一个窗口
        soft_delete(&conn, &c).unwrap(); // 墓碑行不得被归档（否则复活）

        let cutoff = crate::db::iso_days_ago(AUTO_ARCHIVE_DAYS);
        assert_eq!(auto_archive_expired(&conn, &cutoff).unwrap(), vec![a.clone()]);
        assert_eq!(get(&conn, &b).unwrap().unwrap().layer, "buffer");
        assert!(get(&conn, &c).unwrap().is_none());
        // 用户移回缓冲区后不该被下一轮立刻再收走
        set_fragment_layer(&conn, &a, "buffer").unwrap();
        assert!(auto_archive_expired(&conn, &cutoff).unwrap().is_empty());
        assert_eq!(get(&conn, &a).unwrap().unwrap().layer, "buffer");
    }

    #[test]
    fn list_filters_by_layer() {
        let conn = crate::db::test_conn();
        let x = insert(&conn, &nf("缓冲区待办甲")).unwrap();
        let _y = insert(&conn, &nf("缓冲区待办乙")).unwrap();
        set_fragment_layer(&conn, &x.id, "archived").unwrap();
        let buffer = ListFilter { layer: Some("buffer"), ..Default::default() };
        let archived = ListFilter { layer: Some("archived"), ..Default::default() };
        assert_eq!(count_active(&conn, &buffer).unwrap(), 1);
        assert_eq!(count_active(&conn, &archived).unwrap(), 1);
        assert_eq!(list(&conn, 0, 20, &archived).unwrap()[0].id, x.id);
    }

    /// 批次25 计数诚实化：`week_activity.pending`（周回顾「待分拣还有 N 条」那句的数）必须与
    /// 首页 triageCount 同口径——只数**未审的 done+skipped 缓冲区条目**，在途/失败/已归档都不算。
    #[test]
    fn week_pending_counts_only_triage_ready_buffer_rows() {
        let conn = crate::db::test_conn();
        let lease = "2099-01-01T00:00:00.000Z";
        // done：insert 后立即 claim→done，此刻它是唯一 pending，领取不会牵动别的行。
        let done = insert(&conn, &nf("已加工完成等你分拣的一条")).unwrap();
        jobs::claim_next(&conn, "w", lease, 1).unwrap();
        jobs::mark_done(&conn, &done.id).unwrap(); // done：算
        let _gated = insert_skipped(&conn, &nf("被闸门挡下原样收藏的那条")).unwrap(); // skipped：算
        let arch = insert(&conn, &nf("已经放行归档的一条")).unwrap();
        jobs::claim_next(&conn, "w", lease, 1).unwrap();
        jobs::mark_done(&conn, &arch.id).unwrap();
        set_fragment_layer(&conn, &arch.id, "archived").unwrap(); // 离开 buffer，不算
        // 在途 pending 最后插入，永不被领取，故仍是 pending。
        let _inflight = insert(&conn, &nf("刚提交还在排队的条目甲")).unwrap(); // pending：不算
        // 4 条里只有 done + skipped 两条落"待分拣"；在途与已归档各排除一条。
        assert_eq!(
            week_activity(&conn).unwrap().pending,
            2,
            "只数 done+skipped 的未审缓冲区条目，排除在途/已归档"
        );
    }

    /// 「手填标题=明确要整理」的豁免**不适用于隐私**：填标题不是授权把身份证发给模型。
    #[test]
    fn manual_title_exempts_gate_but_never_privacy() {
        assert_eq!(
            assess_skip("有事打我 13800138000", true),
            Some(triage::SkipReason::Sensitive)
        );
        // 普通闸门（裸链接/短记录）在用户填了标题时照常放行
        assert_eq!(assess_skip("https://example.com/a", true), None);
        assert_eq!(assess_skip("四字成语", true), None);
    }

    #[test]
    fn gate_skips_junk_and_honors_manual_title() {
        let conn = crate::db::test_conn();
        // 无标题的裸链接 / 纯符号 → 判跳过；手填标题放行（明确要整理），隐私除外（见上一个测试）
        assert!(assess_skip("https://example.com/x", false).is_some());
        assert!(assess_skip("！！！～", false).is_some());
        assert!(assess_skip("https://example.com/x", true).is_none());
        // 正常长正文不跳过
        assert!(assess_skip("这是一段足够长、值得进 AI 流水线整理的正文内容", false).is_none());
        // insert_skipped 落 skipped，且不在 pending 队列（worker 领不到）
        let f = insert_skipped(&conn, &nf("https://example.com/skipped-me")).unwrap();
        assert_eq!(
            conn.query_row("SELECT status FROM fragment_status WHERE fragment_id=?1", params![&f.id], |r| r.get::<_, String>(0)).unwrap(),
            "skipped"
        );
        assert_eq!(jobs::status_of(&conn, &f.id).unwrap(), Some(jobs::Status::Skipped));
        assert!(jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 5).unwrap().is_empty());
    }

    #[test]
    fn note_is_indexed_for_search() {
        let conn = crate::db::test_conn();
        insert(&conn, &NewFragment {
            content: "正文里没有关键词的片段",
            title: None,
            source: "manual",
            external_url: None,
            media_type: None,
            note: Some("附言提到 量子计算"),
            media_path: None,
            id: None,
        })
        .unwrap();
        assert!(!fts_search(&conn, "量子计算", 10, &[]).unwrap().is_empty());
    }

    #[test]
    fn live_id_set_excludes_tombstones_and_named_layers() {
        let conn = crate::db::test_conn();
        let a = insert(&conn, &nf("正常归档的一条")).unwrap().id;
        let b = insert(&conn, &nf("被丢进垃圾站的一条")).unwrap().id;
        let c = insert(&conn, &nf("将被软删的一条")).unwrap().id;
        conn.execute("UPDATE fragments SET layer='trash' WHERE id=?1", params![&b]).unwrap();
        soft_delete(&conn, &c).unwrap();

        let all = vec![a.clone(), b.clone(), c.clone(), "nonexistent".to_string()];
        let live = live_id_set(&conn, &all, &["trash"]).unwrap();
        assert_eq!(live, std::collections::HashSet::from([a.clone()]));
        // 不限定层时只看墓碑——与 fts_search 同一可见性规则（层口径由调用方给：检索路排除 buffer/trash，02 §3.1）
        assert_eq!(live_id_set(&conn, &all, &[]).unwrap(), std::collections::HashSet::from([a, b]));
        assert!(live_id_set(&conn, &[], &["trash"]).unwrap().is_empty());
    }

    #[test]
    fn set_content_rehashes_and_logs_edit() {
        let conn = crate::db::test_conn();
        let f = insert(&conn, &nf("原始正文内容，被人工改错了一个字")).unwrap();
        assert!(f.content_updated_at.is_none());
        assert_eq!(f.edit_log, "[]");

        let after = set_content(&conn, &f.id, "原始正文内容，被人工改对了一个字").unwrap();
        assert_eq!(after.content, "原始正文内容，被人工改对了一个字");
        // 重算了 hash、推进了修订时刻、记了一条修订账
        assert_eq!(after.content_hash, hash_of(&after.content));
        assert_ne!(after.content_hash, f.content_hash);
        assert!(after.content_updated_at.is_some());
        let log: Vec<EditEntry> = serde_json::from_str(&after.edit_log).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].field, "content");
        assert_eq!(log[0].before_chars, f.char_count);
        assert_eq!(log[0].after_chars, after.char_count);
        assert!(log[0].excerpt.as_deref().unwrap().starts_with('“'));
    }

    #[test]
    fn set_content_unchanged_is_a_noop_and_conflicts_on_dup_hash() {
        let conn = crate::db::test_conn();
        let a = insert(&conn, &nf("一条正常长度的正文用于编辑测试")).unwrap();
        // 改成原样：不落修订账、不推进过期时刻
        let same = set_content(&conn, &a.id, "一条正常长度的正文用于编辑测试").unwrap();
        assert!(same.content_updated_at.is_none());
        assert_eq!(same.edit_log, "[]");
        // 改成与另一条活跃片段一模一样 → 去重索引冲突，拒
        let _b = insert(&conn, &nf("另一条完全不同的正常长度正文内容")).unwrap();
        assert!(matches!(
            set_content(&conn, &a.id, "另一条完全不同的正常长度正文内容"),
            Err(AppError::StateConflict)
        ));
        // 空/超长同样在写边界拒
        assert!(matches!(set_content(&conn, &a.id, "   "), Err(AppError::InputEmpty)));
    }

    #[test]
    fn note_edit_is_logged_but_noop_not_logged() {
        let conn = crate::db::test_conn();
        let f = insert(&conn, &nf("带笔记修订测试的正常长度正文内容")).unwrap();
        // 从无到有 → 记一条
        set_note(&conn, &f.id, Some("第一版附言")).unwrap();
        let g = get(&conn, &f.id).unwrap().unwrap();
        let log: Vec<EditEntry> = serde_json::from_str(&g.edit_log).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].field, "note");
        assert_eq!(log[0].before_chars, 0);
        // 相同内容再设一次 → 无变化，不记
        set_note(&conn, &f.id, Some("第一版附言")).unwrap();
        assert_eq!(get(&conn, &f.id).unwrap().unwrap().edit_log, g.edit_log);
    }

    #[test]
    fn edit_log_caps_at_twenty_dropping_oldest() {
        // 纯函数：反复改到超过上限，只留最近 20 条（防一行 JSON 被撑大）
        let mut existing = "[]".to_string();
        for i in 0..25 {
            let e = EditEntry {
                at: format!("t{i}"),
                field: "content".into(),
                before_chars: 1,
                after_chars: 2,
                excerpt: None,
            };
            existing = edit_log_json(&push_edit(&existing, e));
        }
        let log: Vec<EditEntry> = serde_json::from_str(&existing).unwrap();
        assert_eq!(log.len(), EDIT_LOG_CAP);
        assert_eq!(log.first().unwrap().at, "t5"); // 最旧 5 条被丢
        assert_eq!(log.last().unwrap().at, "t24");
    }

    #[test]
    fn edit_excerpt_withholds_privacy_plaintext() {
        // 含手机号的改前正文：摘录只写占位词，绝不落明文（与 02 §7.1 同口径）
        assert_eq!(edit_excerpt("我的号码是 13800138000 请保存"), EDIT_LOG_WITHHELD);
        assert_eq!(edit_excerpt("短正文"), "“短正文”");
    }
}
