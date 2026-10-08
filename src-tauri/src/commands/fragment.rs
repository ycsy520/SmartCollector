//! 片段 command（02 §1–2）。业务在 db 层，command 只做校验/DTO 组装；
//! `run_skill` 需 LLM（02 §2.7），走 std 线程真调用后回主线程落新版本。
//! `*_impl` 为可脱离 Tauri 单测的纯函数；同名 `#[tauri::command]` 仅做取连接 + 委托。
use rusqlite::{params, Connection};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::db::{fragments, jobs, results, skills};
use crate::dto::fragment as fd;
use crate::error::AppError;
use crate::events::{
    FragmentCreated, FragmentPurged, FragmentUpdated, FRAGMENT_CREATED, FRAGMENT_PURGED,
    FRAGMENT_UPDATED,
};
use crate::state::AppState;

/// 组装详情视图：片段主体 + 处理状态 + 当前/历史结果 + manual 标记。
pub(crate) fn build_detail(conn: &Connection, id: &str) -> Result<fd::FragmentDetail, AppError> {
    let f = fragments::get(conn, id)?.ok_or(AppError::NotFound)?;
    let status = jobs::status_of(conn, id)?
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "pending".into());
    let latest = results::latest(conn, id)?;
    let mut d = fd::detail_base(&f);
    d.status = status;
    d.result = latest.as_ref().map(fd::FragmentResult::from);
    d.prior_results = results::superseded_versions(conn, id)?
        .iter()
        .map(fd::FragmentResult::from)
        .collect();
    d.manual = Some(fd::ManualFlags {
        title: Some(f.title_manual),
        category: latest.as_ref().map(|r| r.category_manual),
        tags: latest.as_ref().map(|r| r.tags_manual),
    });
    // 隐私优先于垃圾：一条含口令的条目不该同时挂两个互相矛盾的徽标。
    // 详情取的是全文，故这里的判定比列表卡片（前 4000 字）更准。
    if let Some(kind) = crate::db::triage::detect_sensitive(&f.content) {
        d.flags.push(fd::sensitive_flag(
            crate::db::triage::sensitive_label(kind),
            d.status == "skipped",
        ));
    } else if d.status == "skipped" && f.media_type != "image" {
        // 闸门判定为垃圾（纯符号/界面文案回捕）时挂徽标；仅记录（裸链接/短记录）不挂，保留归档价值。
        // 图片不在此列（批次21-B）：它的 skipped 是设计上"仅原样收藏、不经模型"（02 §1.3），
        // 挂「低价值」会把它说成被系统判过次的垃圾——占位正文本来就没有内容可判。
        if let Some(reason) = crate::db::triage::assess(&f.content) {
            if reason.is_junk() {
                d.flags.push(fd::FragmentFlag {
                    kind: fd::FragmentFlagKind::Junk,
                    message: "系统判定为低价值信息，已跳过 AI 整理".into(),
                    action: "仍要保留可放行归档，或点「重新处理」强制整理".into(),
                    ref_id: None,
                });
            }
        }
    }
    // 「结果已过期」（批次6-①，读时算）：正文在结果生成之后被人工修订，摘要/向量描述的是旧文本。
    // 只在**有结果**时挂——pending/running 的条目其结果尚未落成，worker 会用新正文处理，不属过期。
    if let (Some(cua), Some(l)) = (&f.content_updated_at, &latest) {
        if cua > &l.processed_at {
            d.flags.push(fd::stale_flag());
        }
    }
    Ok(d)
}

/// 详情 → 列表摘要（excerpt 取正文前 120 字符；flags 收集时另行计算）。
pub(crate) fn to_summary(d: &fd::FragmentDetail) -> fd::FragmentSummary {
    let excerpt: String = d.content.chars().take(120).collect();
    fd::FragmentSummary {
        id: d.id.clone(),
        title: d.title.clone(),
        excerpt,
        status: d.status.clone(),
        category: d.result.as_ref().map(|r| r.category.clone()),
        tags: d.result.as_ref().map(|r| r.tags.clone()).unwrap_or_default(),
        created_at: d.created_at.clone(),
        layer: d.layer.clone(),
        media_type: d.media_type.clone(),
        archived_by: d.archived_by.clone(),
        media_path: d.media_path.clone(),
        flags: d.flags.clone(),
    }
}

fn retry_count(conn: &Connection, id: &str) -> i64 {
    conn.query_row(
        "SELECT retry_count FROM fragment_status WHERE fragment_id = ?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

// ============ 业务实现 ============

pub(crate) fn submit_text_impl(conn: &Connection, input: &fd::SubmitInput) -> Result<fd::SubmitOutcome, AppError> {
    // 幂等收集：同原文已有活跃片段（非垃圾站、未硬删）时不再插入——手工路径此前直撞
    // 唯一索引 uq_frags_active_hash 变成 E_STATE_CONFLICT 红条；改为回执那一条的 id。
    if let Some(existing) = fragments::active_id_by_text(conn, &input.content)? {
        let status = jobs::status_of(conn, &existing)?
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "pending".into());
        return Ok(fd::SubmitOutcome { fragment_id: existing, status, duplicate: true });
    }
    let new = fragments::NewFragment {
        content: &input.content,
        title: input.title.as_deref(),
        source: "manual",
        external_url: None,
        media_type: Some("text"),
        note: input.note.as_deref(),
        media_path: None,
        id: None,
    };
    // 入队前闸门（02 §1.1）：手填标题视为明确要求整理，放行；否则纯启发式判垃圾/仅记录→skipped。
    let (f, status) = if fragments::assess_skip(&input.content, input.title.is_some()).is_some() {
        (fragments::insert_skipped(conn, &new)?, "skipped")
    } else {
        (fragments::insert(conn, &new)?, "pending")
    };
    // skillIds 自动再加工、收集异常 flags 依赖 LLM/worker（P8）；此处仅纯录入入队。
    Ok(fd::SubmitOutcome { fragment_id: f.id, status: status.into(), duplicate: false })
}

/// 02 §1.2：把剪贴板文本收进缓冲区。`text=None` = 剪贴板里没有文本（图片/文件），
/// 与"有文本但全空白"和"库里已有活跃同文"分别对应 `no_text` / `empty` / `duplicate`。
/// 手动触发即明确要收，故不做会话内 seen 环去重（那是自动轮询才需要的防"删后又收"机制）。
/// 收集动作复用 `clip::collect::collect_text`（source=clipboard、纯链接识别、入队前闸门同一条路径）。
pub(crate) fn submit_clipboard_impl(
    conn: &Connection,
    text: Option<&str>,
) -> Result<fd::ClipboardOutcome, AppError> {
    let skipped = |reason: &str| fd::ClipboardOutcome::Skipped { skipped: true, reason: reason.into() };
    let Some(raw) = text else {
        return Ok(skipped("no_text"));
    };
    if raw.trim().is_empty() {
        return Ok(skipped("empty"));
    }
    if fragments::active_id_by_text(conn, raw)?.is_some() {
        return Ok(skipped("duplicate"));
    }
    match crate::clip::collect::collect_text(conn, raw)? {
        // 上面已排空与重复，None 只可能是并发下的同文，按 duplicate 回执而非报错
        None => Ok(skipped("duplicate")),
        Some(id) => {
            let status = jobs::status_of(conn, &id)?
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "pending".into());
            Ok(fd::ClipboardOutcome::Collected(fd::SubmitOutcome {
                fragment_id: id,
                status,
                duplicate: false,
            }))
        }
    }
}

pub(crate) fn get_fragments_impl(conn: &Connection, q: fd::ListQuery) -> Result<fd::Page<fd::FragmentSummary>, AppError> {
    let layer = q.layer.as_deref().unwrap_or("archived"); // 02 §2.1 默认归档层
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let filter = fragments::ListFilter {
        layer: Some(layer),
        status: q.status.as_deref(),
        category: q.category.as_deref(),
        tag: q.tag.as_deref(),
        source: q.source.as_deref(),
        media_type: q.media_type.as_deref(),
        reviewed: q.reviewed,
        archived_by: q.archived_by.as_deref(),
    };
    let items = fragments::list(conn, offset, limit, &filter)?
        .into_iter()
        .map(fd::FragmentSummary::from)
        .collect();
    let total = fragments::count_active(conn, &filter)?;
    Ok(fd::Page { items, total })
}

pub(crate) fn get_fragment_impl(conn: &Connection, id: &str) -> Result<fd::FragmentDetail, AppError> {
    build_detail(conn, id)
}

pub(crate) fn retry_fragment_impl(
    conn: &Connection,
    id: &str,
    confirm_sensitive: bool,
) -> Result<serde_json::Value, AppError> {
    // 批次21-B：图片硬拒。它的"原文"只有一行占位文件名（内容不经模型，02 §1.3），
    // 强制重处理等于把这行文件名发给模型要一篇摘要——产出必然是编造，比空结果更糟。
    ensure_not_image(conn, id, "图片仅原样收藏，「重新处理」对它没有意义（可写附言让它可检索）")?;
    // §2.2 的出口硬查：隐私命中且没带明示确认 → 直接拒。确认值一并落库（outbound_ok），
    // 因为真正外发的是 worker，命令参数活不到那一刻。
    ensure_outbound_confirmed(conn, id, confirm_sensitive)?;
    jobs::manual_retry(conn, id, confirm_sensitive)?; // 仅 failed/done(degraded) 可重试，否则状态冲突/上限
    // pipeline 真重跑由后台 worker 执行（P8 + LLM）；此处重置状态。
    let status = jobs::status_of(conn, id)?.map(|s| s.as_str().to_string()).unwrap_or_else(|| "pending".into());
    Ok(serde_json::json!({ "status": status, "retryCount": retry_count(conn, id) }))
}

/// 隐私闸门（§2.2）：本地规则命中身份证/卡号/手机号/口令时，**默认拒绝**任何把原文交给
/// 外部模型的入口，除非调用方带上用户看过的明示确认。
///
/// 为什么必须在后端而不是只在前端拦：前端拦只是提示，一条 IPC 调用就能绕过；而外发收不回。
/// 错误文案只带类型词（`sensitive_label`），命中的内容片段本身不进消息、不进日志。
/// 确认只在**本次调用**有效——它不由本函数落库，落库在 `manual_retry` 那一侧。
fn ensure_outbound_confirmed(conn: &Connection, id: &str, confirmed: bool) -> Result<(), AppError> {
    if confirmed {
        return Ok(());
    }
    let frag = fragments::get(conn, id)?.ok_or(AppError::NotFound)?;
    match crate::db::triage::detect_sensitive(&frag.content) {
        Some(kind) => Err(AppError::SensitiveConfirm(
            crate::db::triage::sensitive_label(kind).to_string(),
        )),
        None => Ok(()),
    }
}

/// 图片片段闸门：`media_type='image'` 时上抛 E_INPUT_INVALID（带用户可读的说明）。
/// 所有"会把内容交给模型"的入口（重新处理 / 再加工）都要过它——图片的价值恰恰是
/// 原样留着，一旦进流水线就会得到一份凭空调和出来的摘要，而界面无法分辨。
fn ensure_not_image(conn: &Connection, id: &str, why: &str) -> Result<(), AppError> {
    if fragments::get(conn, id)?.map(|f| f.media_type).as_deref() == Some("image") {
        return Err(AppError::InputInvalid(why.into()));
    }
    Ok(())
}

pub(crate) fn delete_fragment_impl(conn: &Connection, id: &str) -> Result<serde_json::Value, AppError> {
    fragments::soft_delete(conn, id)?;
    // 向量行刻意不在这清：墓碑已被两路检索的可见性过滤挡住，孤儿向量由 §2.8 purge 或全量重建收走。
    Ok(serde_json::json!({ "deleted": true }))
}

/// 02 §2.8：彻底删除（物理）。**只接受已在回收站的条目**——否则"删除"这个动词在界面上
/// 就有了第二条、更危险的语义路径（缓冲区/归档里的东西可以一句话物理销毁，绕过 30 天反悔窗口）。
/// 想销毁未进回收站的内容，得先丢弃再彻底删除：两次动作对应两种意图，这是刻意的摩擦。
fn ensure_trashed(conn: &Connection, id: &str) -> Result<(), AppError> {
    let f = fragments::get(conn, id)?.ok_or(AppError::NotFound)?;
    if f.layer != "trash" {
        return Err(AppError::InputInvalid(
            "只能彻底删除回收站里的条目，请先丢弃".into(),
        ));
    }
    Ok(())
}

pub(crate) fn purge_fragment_impl(
    state: &AppState,
    conn: &Connection,
    id: &str,
) -> Result<serde_json::Value, AppError> {
    ensure_trashed(conn, id)?;
    crate::services::purge::purge_one(&state.db, conn, id)?;
    Ok(serde_json::json!({ "purged": true }))
}

pub(crate) fn update_fragment_impl(
    conn: &Connection,
    id: &str,
    patch: &fd::UpdatePatch,
) -> Result<fd::FragmentDetail, AppError> {
    if fragments::get(conn, id)?.is_none() {
        return Err(AppError::NotFound);
    }
    let mut applied = false;
    if let Some(content) = &patch.content {
        fragments::set_content(conn, id, content)?; // 纯本地：改正文不外发（向量删除在 command 封装层做）
        applied = true;
    }
    if let Some(title) = &patch.title {
        fragments::set_title_manual(conn, id, Some(title))?;
        applied = true;
    }
    if let Some(cat) = &patch.category {
        results::set_category_manual(conn, id, cat, None)?;
        applied = true;
    }
    if let Some(tags) = &patch.tags {
        results::set_tags_manual(conn, id, tags)?;
        applied = true;
    }
    if let Some(note) = &patch.note {
        fragments::set_note(conn, id, Some(note))?;
        applied = true;
    }
    if !applied {
        return Err(AppError::InputInvalid("patch 为空".into()));
    }
    build_detail(conn, id) // 向量同步依赖 P6，暂跳过
}

pub(crate) fn set_fragment_layer_impl(conn: &Connection, id: &str, to: &str) -> Result<fd::FragmentDetail, AppError> {
    fragments::set_fragment_layer(conn, id, to)?;
    build_detail(conn, id)
}

// ============ run_skill（02 §2.7，再加工产新版本）============

/// 再加工三条出口的公共前置闸（技能与一次性指令共用，防两处安全判定漂移）：
/// 片段存在（否则 `E_NOT_FOUND`）→ 图片硬拒（`E_INPUT_INVALID`）→ 未确认的隐私拒（`E_SENSITIVE_CONFIRM`）
/// → 仅 `done` 可加工（否则 `E_STATE_CONFLICT`）。通过返回片段行供调用方取正文/媒体类型。
/// 04 §2.2 出口硬查放这里：技能是一次性指令的唯一同步出口，确认之后当场即发，确认值不落库
/// （落库反把一次确认变永久授权）。
fn precheck_runnable(
    conn: &Connection,
    fragment_id: &str,
    confirm_sensitive: bool,
) -> Result<crate::db::fragments::Fragment, AppError> {
    let frag = fragments::get(conn, fragment_id)?.ok_or(AppError::NotFound)?;
    // 批次21-B：图片不进再加工。内置技能的 media_type 是 'all'，媒体门控拦不住它，
    // 必须在这里单独硬拒——否则一张图会得到一段拿占位文件名编出来的"技能摘要"。
    if frag.media_type == "image" {
        return Err(AppError::InputInvalid(
            "图片仅原样收藏，不参与再加工（可写附言让它可检索）".into(),
        ));
    }
    if !confirm_sensitive {
        if let Some(kind) = crate::db::triage::detect_sensitive(&frag.content) {
            return Err(AppError::SensitiveConfirm(
                crate::db::triage::sensitive_label(kind).to_string(),
            ));
        }
    }
    if jobs::status_of(conn, fragment_id)? != Some(jobs::Status::Done) {
        return Err(AppError::StateConflict); // 仅 done 可再加工
    }
    Ok(frag)
}

/// 离线闸门：片段可再加工（见 `precheck_runnable`）+ 技能存在 + `media_type × category` 门控通过。
/// 通过则返回（已填充的 skill prompt、技能名）。技能未找到 → `E_NOT_FOUND`；门控不过 → `E_STATE_CONFLICT`。
/// category 门控取当前结果的分类（fragments 表无分类列）。
pub(crate) fn check_skill_run_impl(
    conn: &Connection,
    fragment_id: &str,
    skill_id: &str,
    confirm_sensitive: bool,
) -> Result<(String, String), AppError> {
    let frag = precheck_runnable(conn, fragment_id, confirm_sensitive)?;
    let skill = skills::get(conn, skill_id)?.ok_or(AppError::NotFound)?;
    let cur_category = results::latest(conn, fragment_id)?
        .map(|r| r.category)
        .unwrap_or_else(|| "其他".into());
    if skill.media_type != "all" && skill.media_type != frag.media_type {
        return Err(AppError::StateConflict);
    }
    if skill.category != "all" && skill.category != cur_category {
        return Err(AppError::StateConflict);
    }
    let prompt = crate::agent::prompt::build_skill(&skill.prompt, &frag.content);
    Ok((prompt, skill.name))
}

/// 一次性指令闸门（02 §2.7 `instruction` 分支）：过公共前置闸后，用现场指令当技能正文拼 prompt，
/// 不进技能库、不落库。走与技能完全相同的隐私/图片/`done` 闸门——差别只在"正文来自当场输入而非库里某行"。
/// 指令为空白 → `E_INPUT_INVALID`。返回（已填充的 prompt、呈现名）。
pub(crate) fn check_instruction_run_impl(
    conn: &Connection,
    fragment_id: &str,
    instruction: &str,
    confirm_sensitive: bool,
) -> Result<(String, String), AppError> {
    let frag = precheck_runnable(conn, fragment_id, confirm_sensitive)?;
    if instruction.trim().is_empty() {
        return Err(AppError::InputInvalid("一次性指令不能为空".into()));
    }
    let prompt = crate::agent::prompt::build_skill(instruction, &frag.content);
    Ok((prompt, INSTALLED_INSTRUCTION_NAME.to_string()))
}

/// 一次性指令在版本历史里的呈现名（`model_used = "skill:{此名}"`，前端按 `skill:` 前缀解析）。
const INSTALLED_INSTRUCTION_NAME: &str = "一次性指令";

/// 落新版本：技能只改摘要，沿用 prior 的 category/subcategory/tags/links；
/// `model_used="skill:{name}"`；不改动人工 `manual` 标记（把 prior 的 manual 标志与值一并带到新版本）。
/// 缺 `data.summary` → `E_LLM_BAD_OUTPUT`（不回写、不产空版本）。返回新版本号。
pub(crate) fn apply_skill_result(
    conn: &Connection,
    fragment_id: &str,
    skill_name: &str,
    data: &Value,
) -> Result<i64, AppError> {
    let summary = data
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::LlmBadOutput("技能输出缺少 summary".into()))?;
    let prior = results::latest(conn, fragment_id)?;
    let (category, subcategory, tags, links) = match &prior {
        Some(p) => (p.category.clone(), p.subcategory.clone(), p.tags.clone(), p.links.clone()),
        None => (String::from("其他"), None, Vec::new(), Vec::new()), // done 却无结果：兜底，理论不达
    };
    let res = results::insert_result(
        conn,
        fragment_id,
        &category,
        subcategory.as_deref(),
        &tags,
        summary,
        &links,
        false,
        Some(&format!("skill:{skill_name}")),
    )?;
    if let Some(p) = &prior {
        if p.category_manual || p.tags_manual {
            conn.execute(
                "UPDATE processing_results SET category_manual = ?2, tags_manual = ?3 WHERE id = ?1",
                params![res.id, p.category_manual as i64, p.tags_manual as i64],
            )
            .map_err(|e| AppError::DbWrite(e.to_string()))?;
        }
    }
    Ok(res.version)
}

// ============ #[tauri::command] 薄封装 ============

macro_rules! delegate {
    ($state:expr, $f:expr) => {{
        let conn = $state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        ($f)(&conn)
    }};
}

#[tauri::command]
pub fn submit_text(
    state: State<'_, AppState>,
    app: AppHandle,
    input: fd::SubmitInput,
) -> Result<fd::SubmitOutcome, AppError> {
    let outcome = delegate!(state, |c: &Connection| submit_text_impl(c, &input))?;
    if !outcome.duplicate {
        let _ = app.emit(
            FRAGMENT_CREATED,
            FragmentCreated { fragment_id: outcome.fragment_id.clone(), source: "manual" },
        );
    }
    Ok(outcome)
}

/// 02 §1.2：**用户主动**触发（托盘「收集剪贴板」/前端按钮）时才读一次剪贴板。无后台轮询。
/// 剪贴板根本打不开（无访问权限、会话异常）才算 `E_CLIPBOARD_READ`；能打开但读不出文本
/// （当前是图片/文件）按契约回落 `{skipped:true, reason:"no_text"}`，不是错误。
#[tauri::command]
pub fn submit_clipboard(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<fd::ClipboardOutcome, AppError> {
    let text = arboard::Clipboard::new()
        .map_err(|_| AppError::ClipboardRead)?
        .get_text()
        .ok();
    let outcome = delegate!(state, |c: &Connection| submit_clipboard_impl(c, text.as_deref()))?;
    if let fd::ClipboardOutcome::Collected(o) = &outcome {
        let _ = app.emit(
            FRAGMENT_CREATED,
            FragmentCreated { fragment_id: o.fragment_id.clone(), source: "clipboard" },
        );
    }
    Ok(outcome)
}

#[tauri::command]
pub fn get_fragments(state: State<'_, AppState>, query: Option<fd::ListQuery>) -> Result<fd::Page<fd::FragmentSummary>, AppError> {
    delegate!(state, |c: &Connection| get_fragments_impl(c, query.unwrap_or_default()))
}

#[tauri::command]
pub fn get_fragment(state: State<'_, AppState>, fragment_id: String) -> Result<fd::FragmentDetail, AppError> {
    delegate!(state, |c: &Connection| get_fragment_impl(c, &fragment_id))
}

#[tauri::command]
pub fn retry_fragment(
    state: State<'_, AppState>,
    fragment_id: String,
    confirm_sensitive: bool,
) -> Result<serde_json::Value, AppError> {
    delegate!(state, |c: &Connection| retry_fragment_impl(c, &fragment_id, confirm_sensitive))
}

#[tauri::command]
pub fn delete_fragment(state: State<'_, AppState>, fragment_id: String) -> Result<serde_json::Value, AppError> {
    delegate!(state, |c: &Connection| delete_fragment_impl(c, &fragment_id))
}

#[tauri::command]
pub fn purge_fragment(
    state: State<'_, AppState>,
    app: AppHandle,
    fragment_id: String,
) -> Result<serde_json::Value, AppError> {
    let r = delegate!(state, |c: &Connection| purge_fragment_impl(state.inner(), c, &fragment_id))?;
    // 发 purged 而非 updated：前端拿 updated 会去 fetch 一条已不存在的行，把 NotFound 当成错误弹给用户。
    let _ = app.emit(FRAGMENT_PURGED, FragmentPurged { fragment_id });
    Ok(r)
}

#[tauri::command]
pub fn update_fragment(
    state: State<'_, AppState>,
    app: AppHandle,
    fragment_id: String,
    patch: fd::UpdatePatch,
) -> Result<fd::FragmentDetail, AppError> {
    let d = delegate!(state, |c: &Connection| update_fragment_impl(c, &fragment_id, &patch))?;
    // 改了正文：旧向量描述的是改前文本，编辑又不该外发（不重算 embedding），故只删不补——
    // 语义检索会暂时缺这一条，代价远小于擅自外发；下次「重新处理」或全量重建再收回来。删除不联网。
    if patch.content.is_some() {
        if let Ok(conn) = state.db.get() {
            crate::services::purge::purge_vector(&state.db, &conn, &fragment_id);
        }
        // 正文变了要广播：别的窗口/列表据此刷新摘要与「已过期」标记（本窗口经命令回执已 upsert）。
        let _ = app.emit(FRAGMENT_UPDATED, FragmentUpdated { fragment_id });
    }
    Ok(d)
}

#[tauri::command]
pub fn set_fragment_layer(
    state: State<'_, AppState>,
    app: AppHandle,
    fragment_id: String,
    to: String,
) -> Result<fd::FragmentDetail, AppError> {
    let d = delegate!(state, |c: &Connection| set_fragment_layer_impl(c, &fragment_id, &to))?;
    let _ = app.emit(FRAGMENT_UPDATED, FragmentUpdated { fragment_id });
    Ok(d)
}

/// 02 §2.7：对 done 片段跑某技能再加工，产出新结果版本。闸门与写回均可离线测；
/// 真 LLM 调用放独立 std 线程（blocking reqwest 不能在 tauri 运行时线程上跑，见 worker 头注释）。
/// `skill_id` 与 `instruction` 二选一：给了 `instruction` 走一次性指令（不进库），否则按 `skill_id` 跑既有技能。
#[tauri::command]
pub fn run_skill(
    state: State<'_, AppState>,
    app: AppHandle,
    fragment_id: String,
    skill_id: Option<String>,
    instruction: Option<String>,
    confirm_sensitive: bool,
) -> Result<fd::SkillOutcome, AppError> {
    let (prompt, skill_name, cfg) = {
        let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        let (prompt, skill_name) = match instruction.as_deref().map(str::trim) {
            Some(text) if !text.is_empty() => {
                check_instruction_run_impl(&conn, &fragment_id, text, confirm_sensitive)?
            }
            _ => {
                let sid = skill_id.as_deref().ok_or_else(|| {
                    AppError::InputInvalid("技能再加工需给出 skill_id 或 instruction".into())
                })?;
                check_skill_run_impl(&conn, &fragment_id, sid, confirm_sensitive)?
            }
        };
        (prompt, skill_name, crate::db::config_store::load(&conn)?)
    };
    run_skill_exec(&state, &app, &fragment_id, &prompt, &skill_name, &cfg)
}

/// 装配 LLM 客户端并执行一次技能调用 → 落版本 → 发 `fragment://updated`。
/// 未配置（缺模型/密钥）→ `E_CONFIG_MISSING`（全局码 02 §6；done 片段通常意味着已配置）。
fn run_skill_exec(
    state: &State<'_, AppState>,
    app: &AppHandle,
    fragment_id: &str,
    prompt: &str,
    skill_name: &str,
    cfg: &crate::config::AppConfig,
) -> Result<fd::SkillOutcome, AppError> {
    use crate::config::secrets;
    // 总开关关闭 = 大模型不可用，与"未配置"同样拒（不烧 token、不越权外发）；前端在关时已禁用按钮，此为后端兜底。
    if !(cfg.llm_enabled && cfg.has_primary_llm() && secrets::api_key_for(&cfg.llm_primary.provider).is_some()) {
        return Err(AppError::ConfigMissing);
    }
    let endpoint = cfg.llm_primary.clone();
    let fallback = cfg.llm_fallback.clone();
    let api_key = secrets::api_key_for(&endpoint.provider).unwrap_or_default();
    let fallback_key = fallback.as_ref().and_then(|e| secrets::api_key_for(&e.provider));
    let retry = cfg.agent_retry.clone();
    let prompt_owned = prompt.to_string();
    // 计量要跨线程落库，故记录器持池（连接不是 Send）。
    let pool = state.db.clone();

    // 独立 std 线程内建 blocking 客户端 + 跑一次，避免 nested-runtime panic。
    let handle = std::thread::spawn(move || {
        use crate::db::usage::DbUsageRecorder;
        use crate::llm::openai::HttpTransport;
        use crate::llm::{LlmRouter, RouterClient};
        let transport = HttpTransport::new();
        let recorder = DbUsageRecorder { pool };
        let router = LlmRouter::new(&transport, endpoint, fallback, api_key, fallback_key, retry)
            .with_usage(&recorder);
        let client = RouterClient { router: &router, task: "skill_run" };
        crate::agent::call_json_checked(&client, "skill_run", &prompt_owned)
    });
    let data = match handle.join() {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(AppError::Internal("技能处理线程异常退出".into())),
    };

    let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
    let new_version = apply_skill_result(&conn, fragment_id, skill_name, &data)?;
    let _ = app.emit(FRAGMENT_UPDATED, FragmentUpdated { fragment_id: fragment_id.to_string() });
    Ok(fd::SkillOutcome { new_version })
}

// TODO(P8): submit_link（reqwest 抓取 + 正文抽取策略，见 待确认清单）尚未实现。

#[cfg(test)]
mod tests {
    use super::*;

    fn submit(conn: &Connection, text: &str) -> String {
        submit_text_impl(
            conn,
            &fd::SubmitInput { content: text.into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap()
        .fragment_id
    }

    #[test]
    fn submit_lands_in_buffer_and_lists_there() {
        let conn = crate::db::test_conn();
        let id = submit(&conn, "第一条缓冲区内的正常长度内容");
        let buffer = get_fragments_impl(&conn, fd::ListQuery { layer: Some("buffer".into()), ..Default::default() }).unwrap();
        assert_eq!(buffer.total, 1);
        assert_eq!(buffer.items[0].id, id);
        assert_eq!(buffer.items[0].status, "pending");
        // 默认查询只看归档层 → 空
        let archived = get_fragments_impl(&conn, fd::ListQuery::default()).unwrap();
        assert_eq!(archived.total, 0);
    }

    #[test]
    fn duplicate_submit_is_idempotent_not_an_error() {
        let conn = crate::db::test_conn();
        let text = "同一段正常长度的内容被收了两次";
        let first = submit(&conn, text);
        let again = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: text.into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap();
        assert!(again.duplicate);
        assert_eq!(again.fragment_id, first);
        let buffer = get_fragments_impl(&conn, fd::ListQuery { layer: Some("buffer".into()), ..Default::default() }).unwrap();
        assert_eq!(buffer.total, 1); // 没有第二条
    }

    #[test]
    fn discarded_fragment_can_be_collected_again() {
        let conn = crate::db::test_conn();
        let text = "被丢进垃圾站后又重新收集的长内容";
        let first = submit(&conn, text);
        set_fragment_layer_impl(&conn, &first, "trash").unwrap();
        let again = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: text.into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap();
        // 垃圾站不占去重槽（与 uq_frags_active_hash 同语义）：这是一条全新的收集
        assert!(!again.duplicate);
        assert_ne!(again.fragment_id, first);
    }

    #[test]
    fn clipboard_outcome_distinguishes_no_text_empty_and_duplicate() {
        let conn = crate::db::test_conn();
        let reason = |r: fd::ClipboardOutcome| match r {
            fd::ClipboardOutcome::Skipped { skipped, reason } => {
                assert!(skipped);
                Some(reason)
            }
            fd::ClipboardOutcome::Collected(_) => None,
        };
        assert_eq!(reason(submit_clipboard_impl(&conn, None).unwrap()).as_deref(), Some("no_text"));
        assert_eq!(reason(submit_clipboard_impl(&conn, Some("  \n ")).unwrap()).as_deref(), Some("empty"));
        let text = "剪贴板里复制来的一段正常正文内容";
        let collected = submit_clipboard_impl(&conn, Some(text)).unwrap();
        let id = match &collected {
            fd::ClipboardOutcome::Collected(o) => o.fragment_id.clone(),
            fd::ClipboardOutcome::Skipped { .. } => panic!("应收集"),
        };
        assert_eq!(reason(collected), None);
        let d = get_fragment_impl(&conn, &id).unwrap();
        assert_eq!(d.source, "clipboard");
        assert_eq!(d.layer, "buffer");
        assert_eq!(reason(submit_clipboard_impl(&conn, Some(text)).unwrap()).as_deref(), Some("duplicate"));
        assert_eq!(get_fragments_impl(&conn, fd::ListQuery { layer: Some("buffer".into()), ..Default::default() }).unwrap().total, 1);
    }

    #[test]
    fn layer_transition_moves_between_lists() {
        let conn = crate::db::test_conn();
        let id = submit(&conn, "待放行的片段");
        let d = set_fragment_layer_impl(&conn, &id, "archived").unwrap();
        assert_eq!(d.layer, "archived");
        assert_eq!(d.archived_by.as_deref(), Some("manual"));
        assert!(d.reviewed);
        let archived = get_fragments_impl(&conn, fd::ListQuery::default()).unwrap();
        assert_eq!(archived.items.len(), 1);
        assert_eq!(archived.items[0].archived_by.as_deref(), Some("manual"));
    }

    #[test]
    fn purge_only_accepts_trashed_fragments() {
        let conn = crate::db::test_conn();
        let in_buffer = submit(&conn, "还在缓冲区的内容");
        let in_archive = submit(&conn, "已放行的内容");
        set_fragment_layer_impl(&conn, &in_archive, "archived").unwrap();
        let in_trash = submit(&conn, "已丢弃的内容");
        set_fragment_layer_impl(&conn, &in_trash, "trash").unwrap();

        // 未进回收站的两条：拒绝，且条目还在（"彻底删除"必须是第二次动作）
        for id in [&in_buffer, &in_archive] {
            assert!(matches!(ensure_trashed(&conn, id), Err(AppError::InputInvalid(_))));
            assert!(fragments::get(&conn, id).unwrap().is_some());
        }
        assert!(matches!(ensure_trashed(&conn, "no-such-id"), Err(AppError::NotFound)));
        assert!(ensure_trashed(&conn, &in_trash).is_ok());
    }

    #[test]
    fn update_fragment_marks_manual_and_searchable() {
        let conn = crate::db::test_conn();
        let id = submit(&conn, "关于 Rust 所有权的内容");
        let d = update_fragment_impl(
            &conn,
            &id,
            &fd::UpdatePatch { content: None, title: Some("手改标题".into()), category: Some("技术".into()), tags: Some(vec!["所有权".into()]), note: None },
        )
        .unwrap();
        assert_eq!(d.title.as_deref(), Some("手改标题"));
        assert_eq!(d.result.as_ref().unwrap().category, "技术");
        let m = d.manual.unwrap();
        assert_eq!(m.title, Some(true));
        assert_eq!(m.category, Some(true));
        assert_eq!(m.tags, Some(true));
        // get_fragment 读回一致
        assert_eq!(get_fragment_impl(&conn, &id).unwrap().id, id);
        // 空 patch 报错
        assert!(matches!(
            update_fragment_impl(&conn, &id, &fd::UpdatePatch::default()),
            Err(AppError::InputInvalid(_))
        ));
    }

    #[test]
    fn update_fragment_sets_and_clears_note() {
        let conn = crate::db::test_conn();
        let id = submit(&conn, "附言测试正文");
        // 设置附言
        let d = update_fragment_impl(&conn, &id, &fd::UpdatePatch { note: Some("我的备注".into()), ..Default::default() }).unwrap();
        assert_eq!(d.note.as_deref(), Some("我的备注"));
        // note 进 FTS，可被关键词命中
        assert!(!fragments::fts_search(&conn, "我的备注", 10, &[]).unwrap().is_empty());
        // 空串清除
        let d2 = update_fragment_impl(&conn, &id, &fd::UpdatePatch { note: Some("   ".into()), ..Default::default() }).unwrap();
        assert_eq!(d2.note, None);
        assert!(fragments::fts_search(&conn, "我的备注", 10, &[]).unwrap().is_empty());
    }

    #[test]
    fn content_edit_on_done_fragment_raises_stale_flag_and_log() {
        let conn = crate::db::test_conn();
        let id = done_fragment(&conn, "改前的正文将被修订一段较长内容", "技术");
        // 钉死改前结果时刻，确保改后 content_updated_at 必然更大（now_iso 毫秒粒度，避免同毫秒抖动）
        conn.execute("UPDATE processing_results SET processed_at='2000-01-01T00:00:00.000Z' WHERE fragment_id=?1", rusqlite::params![&id]).unwrap();
        let d = update_fragment_impl(&conn, &id, &fd::UpdatePatch { content: Some("改后的新正文完全不同的一段话".into()), ..Default::default() }).unwrap();
        assert!(d.flags.iter().any(|f| matches!(f.kind, fd::FragmentFlagKind::Stale)));
        assert_eq!(d.edit_log.len(), 1);
        assert_eq!(d.edit_log[0].field, "content");
        // 读回正文已更新，改前原文已不在 content
        assert_eq!(get_fragment_impl(&conn, &id).unwrap().content, "改后的新正文完全不同的一段话");
    }

    #[test]
    fn list_summary_carries_stale_and_junk_flags_matching_detail() {
        // #3-P0：get_fragments 列表摘要不再只有 sensitive——stale/junk 要与 detail 口径一致，
        // 让整理模式/卡片在摘要态即可见徽标（前端 store 拆分后 index 只带摘要，旗标不能丢）。
        let conn = crate::db::test_conn();
        let id = done_fragment(&conn, "改前的一段较长正文用于制造差异", "技术");
        conn.execute(
            "UPDATE processing_results SET processed_at='2000-01-01T00:00:00.000Z' WHERE fragment_id=?1",
            rusqlite::params![&id],
        )
        .unwrap();
        update_fragment_impl(
            &conn,
            &id,
            &fd::UpdatePatch { content: Some("改后完全不同的一段新正文内容".into()), ..Default::default() },
        )
        .unwrap();
        // 该 done 片段仍在缓冲区，用 buffer 层取列表摘要
        let page = get_fragments_impl(
            &conn,
            fd::ListQuery { layer: Some("buffer".into()), ..Default::default() },
        )
        .unwrap();
        let s = page.items.iter().find(|x| x.id == id).expect("应在列表摘要里");
        assert!(s.flags.iter().any(|f| f.kind == fd::FragmentFlagKind::Stale), "摘要应带已修订旗标");

        // 纯符号垃圾 → skipped → 摘要带 junk 旗标
        let junk = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "！！！～～～".into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap();
        let page2 = get_fragments_impl(
            &conn,
            fd::ListQuery { layer: Some("buffer".into()), ..Default::default() },
        )
        .unwrap();
        let js = page2.items.iter().find(|x| x.id == junk.fragment_id).expect("垃圾应在列表摘要里");
        assert!(js.flags.iter().any(|f| f.kind == fd::FragmentFlagKind::Junk), "摘要应带低价值旗标");
    }

    #[test]
    fn gate_skips_junk_at_submit_and_flags_in_detail() {
        let conn = crate::db::test_conn();
        // 纯符号垃圾：无标题提交 → 落 skipped，worker 领不到
        let junk = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "！！！～～～".into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap();
        assert_eq!(junk.status, "skipped");
        assert!(jobs::claim_next(&conn, "w", "2099-01-01T00:00:00.000Z", 5).unwrap().is_empty());
        let d = get_fragment_impl(&conn, &junk.fragment_id).unwrap();
        assert_eq!(d.status, "skipped");
        assert!(d.flags.iter().any(|f| f.kind == fd::FragmentFlagKind::Junk), "垃圾应挂徽标");
        // 裸链接=仅记录：skipped 但不判垃圾（保留归档价值）
        let link = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "https://example.com/keep".into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap();
        assert_eq!(link.status, "skipped");
        let dl = get_fragment_impl(&conn, &link.fragment_id).unwrap();
        assert!(!dl.flags.iter().any(|f| f.kind == fd::FragmentFlagKind::Junk), "仅记录不判垃圾");
        // 手填标题 → 强制进队列（pending），闸门让路
        let titled = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "短".into(), title: Some("我要整理它".into()), note: None, skill_ids: vec![] },
        )
        .unwrap();
        assert_eq!(titled.status, "pending");
    }

    #[test]
    fn retry_allowed_on_skipped_forces_processing() {
        let conn = crate::db::test_conn();
        let id = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "～！～！".into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap()
        .fragment_id;
        assert_eq!(jobs::status_of(&conn, &id).unwrap(), Some(jobs::Status::Skipped));
        let out = retry_fragment_impl(&conn, &id, false).unwrap();
        assert_eq!(out["status"], serde_json::json!("pending"));
    }

    #[test]
    fn retry_of_sensitive_fragment_requires_explicit_confirm() {
        // 审查 P0-1（§2.2）：入队闸门挡住隐私条目的同时，「重新处理」这个出口不得一键放行。
        let conn = crate::db::test_conn();
        let id = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "有事打我 13800138000 这台是备用机".into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap()
        .fragment_id;
        assert_eq!(jobs::status_of(&conn, &id).unwrap(), Some(jobs::Status::Skipped));
        // 未确认 → 硬拒；拒绝不得顺手放开状态（否则报错也成了"半个成功"）
        let err = retry_fragment_impl(&conn, &id, false).unwrap_err();
        assert!(matches!(err, AppError::SensitiveConfirm(_)));
        assert_eq!(jobs::status_of(&conn, &id).unwrap(), Some(jobs::Status::Skipped));
        assert!(!jobs::outbound_allowed(&conn, &id));
        // 错误信息只带类型词：命中的号码绝不回显（它会被写进日志、被前端原样渲染）
        assert!(!err.to_string().contains("13800138000"));
        // 明示确认 → 放行，且授权落库：真正把正文发出去的是 worker 的下一次调用，参数活不到那时
        assert_eq!(retry_fragment_impl(&conn, &id, true).unwrap()["status"], serde_json::json!("pending"));
        assert!(jobs::outbound_allowed(&conn, &id));
    }

    #[test]
    fn editing_content_revokes_the_previous_outbound_authorization() {
        // 一次确认只买"当时那份文本"的一次外发。改过正文就作废，否则用户确认的是旧号码，
        // 发出去的是从没被看过第二眼的新密码。
        let conn = crate::db::test_conn();
        let id = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "有事打我 13800138000 这台是备用机".into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap()
        .fragment_id;
        retry_fragment_impl(&conn, &id, true).unwrap();
        assert!(jobs::outbound_allowed(&conn, &id));
        update_fragment_impl(
            &conn,
            &id,
            &fd::UpdatePatch { content: Some("换成另一串数字 13900139000 这是新号码".into()), ..Default::default() },
        )
        .unwrap();
        assert!(!jobs::outbound_allowed(&conn, &id), "改正文后授权必须作废");
    }

    #[test]
    fn non_sensitive_skipped_retry_needs_no_confirm_and_earns_no_authorization() {
        // 反向验证：闸门不能把普通条目也拦下（否则「强制处理」等于被废掉），
        // 也不能给没确认过的条目留下授权位——skipped 有两种，只有隐私那种才要确认。
        let conn = crate::db::test_conn();
        let id = submit_text_impl(
            &conn,
            &fd::SubmitInput { content: "～！～！".into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap()
        .fragment_id;
        assert_eq!(jobs::status_of(&conn, &id).unwrap(), Some(jobs::Status::Skipped));
        assert_eq!(retry_fragment_impl(&conn, &id, false).unwrap()["status"], serde_json::json!("pending"));
        assert!(!jobs::outbound_allowed(&conn, &id));
    }

    #[test]
    fn delete_removes_from_reads() {
        let conn = crate::db::test_conn();
        let id = submit(&conn, "将被软删的内容");
        delete_fragment_impl(&conn, &id).unwrap();
        assert!(matches!(get_fragment_impl(&conn, &id), Err(AppError::NotFound)));
    }

    #[test]
    fn retry_only_allowed_on_failed() {
        let conn = crate::db::test_conn();
        let id = submit(&conn, "仍处于待办的正常长度内容不能被重试");
        // pending 状态不可重试
        assert!(retry_fragment_impl(&conn, &id, false).is_err());
    }

    #[test]
    fn detail_exposes_prior_versions_after_reprocess() {
        let conn = crate::db::test_conn();
        let id = submit(&conn, "这条会被再加工处理一段较长内容");
        // 直接写两版结果模拟 pipeline/skill 产出（第一版被 superseded）
        results::insert_result(&conn, &id, "技术", None, &["a".into()], "旧摘要", &[], false, None).unwrap();
        results::insert_result(&conn, &id, "技术", None, &["b".into()], "新摘要", &[], false, None).unwrap();
        let d = get_fragment_impl(&conn, &id).unwrap();
        assert_eq!(d.result.unwrap().summary, "新摘要");
        assert_eq!(d.prior_results.len(), 1);
        assert_eq!(d.prior_results[0].summary, "旧摘要");
    }

    // ============ run_skill ============

    /// 把片段推进到 done（模拟 worker 完成），返回片段 id。
    fn done_fragment(conn: &Connection, text: &str, category: &str) -> String {
        let id = submit(conn, text);
        jobs::claim_next(conn, "w", "2099-01-01T00:00:00.000Z", 1).unwrap();
        jobs::mark_done(conn, &id).unwrap();
        results::insert_result(conn, &id, category, Some("子"), &["旧标签".into()], "基础摘要", &[], false, Some("m")).unwrap();
        id
    }

    #[test]
    fn skill_gate_rejects_when_not_done() {
        let conn = crate::db::test_conn();
        skills::ensure_builtin(&conn).unwrap();
        let id = submit(&conn, "还停在待处理队列中的正常片段"); // 未 done
        assert!(matches!(
            check_skill_run_impl(&conn, &id, "builtin:verify", false),
            Err(AppError::StateConflict)
        ));
    }

    #[test]
    fn skill_gate_rejects_missing_fragment_and_skill() {
        let conn = crate::db::test_conn();
        skills::ensure_builtin(&conn).unwrap();
        let id = done_fragment(&conn, "已经完成处理的正常长片段内容", "技术");
        // 技能不存在
        assert!(matches!(check_skill_run_impl(&conn, &id, "no-such", false), Err(AppError::NotFound)));
        // 片段不存在
        assert!(matches!(check_skill_run_impl(&conn, "ghost", "builtin:verify", false), Err(AppError::NotFound)));
    }

    #[test]
    fn skill_gate_checks_media_and_category() {
        let conn = crate::db::test_conn();
        skills::ensure_builtin(&conn).unwrap();
        let id = done_fragment(&conn, "这是技术类已经完成的长片段", "技术");
        // builtin:verify 是 all×all → 放行，返回填充好的 prompt（含技能正文与原文）
        let (prompt, name) = check_skill_run_impl(&conn, &id, "builtin:verify", false).unwrap();
        assert_eq!(name, "验证真伪");
        assert!(prompt.contains("判断片段中的关键事实陈述"));
        assert!(prompt.contains("技术类已经完成的长片段"));
        // 自定义：category=生活 × 当前结果 category=技术 → 不匹配
        let lifestyle = skills::Skill {
            id: String::new(), name: "生活技能".into(), builtin: false, trigger: "manual".into(),
            media_type: "all".into(), category: "生活".into(), prompt: "做点啥".into(), enabled: true,
        };
        let s = skills::upsert(&conn, &lifestyle).unwrap();
        assert!(matches!(check_skill_run_impl(&conn, &id, &s.id, false), Err(AppError::StateConflict)));
    }

    #[test]
    fn skill_gate_requires_confirm_for_sensitive_content() {
        // 审查 P0-1：技能是"把原文交给模型"的第二条出口，同样要过确认闸。
        let conn = crate::db::test_conn();
        skills::ensure_builtin(&conn).unwrap();
        let id = done_fragment(&conn, "这是一段已经处理完成的正常长正文", "技术");
        // 正文含隐私（模拟"结果已就绪、文本里有号码"这条再加工路径）
        conn.execute(
            "UPDATE fragments SET content=?2 WHERE id=?1",
            rusqlite::params![&id, "有事打我 13800138000 这段正文里含手机号"],
        )
        .unwrap();
        assert!(matches!(
            check_skill_run_impl(&conn, &id, "builtin:verify", false),
            Err(AppError::SensitiveConfirm(_))
        ));
        // 技能是同步调用：确认之后当场就发，所以不需要授权位落库（落库反而把一次确认变永久）
        let (prompt, _) = check_skill_run_impl(&conn, &id, "builtin:verify", true).unwrap();
        assert!(prompt.contains("13800138000"), "确认后 prompt 才带上原文");
    }

    // ============ 一次性指令（02 §2.7 instruction 分支）============

    #[test]
    fn instruction_run_goes_through_same_gates_as_skill() {
        let conn = crate::db::test_conn();
        let id = done_fragment(&conn, "这是技术类已经完成的长片段", "技术");
        // 空白指令 → E_INPUT_INVALID
        assert!(matches!(
            check_instruction_run_impl(&conn, &id, "   ", false),
            Err(AppError::InputInvalid(_))
        ));
        // 未 done 的片段 → E_STATE_CONFLICT（与技能同一前置闸）
        let pending = submit(&conn, "还停在待处理队列中的正常片段");
        assert!(matches!(
            check_instruction_run_impl(&conn, &pending, "换个角度重写", false),
            Err(AppError::StateConflict)
        ));
        // 正常指令：返回以指令当技能正文填好的 prompt，呈现名固定「一次性指令」
        let (prompt, name) = check_instruction_run_impl(&conn, &id, "把它改写成一问一答", false).unwrap();
        assert_eq!(name, INSTALLED_INSTRUCTION_NAME);
        assert!(prompt.contains("把它改写成一问一答"), "prompt 带上现场指令");
        assert!(prompt.contains("技术类已经完成的长片段"), "prompt 带上原文");
    }

    #[test]
    fn instruction_run_requires_confirm_for_sensitive_content() {
        // 一次性指令同样是"把原文交给模型"的出口，必须过隐私确认闸——不因"临时"而跳过。
        let conn = crate::db::test_conn();
        let id = done_fragment(&conn, "这是一段已经处理完成的正常长正文", "技术");
        conn.execute(
            "UPDATE fragments SET content=?2 WHERE id=?1",
            rusqlite::params![&id, "有事打我 13800138000 这段正文里含手机号"],
        )
        .unwrap();
        assert!(matches!(
            check_instruction_run_impl(&conn, &id, "总结要点", false),
            Err(AppError::SensitiveConfirm(_))
        ));
        assert!(check_instruction_run_impl(&conn, &id, "总结要点", true).is_ok());
    }

    #[test]
    fn skill_result_writes_new_version_and_inherits_manual() {
        let conn = crate::db::test_conn();
        let id = done_fragment(&conn, "这条会被技能再加工处理的长片段", "技术");
        // 用户手工改过分类 → manual 标记应跨版本沿用
        results::set_category_manual(&conn, &id, "学习", Some("笔记")).unwrap();
        let data: Value = serde_json::json!({ "summary": "技能生成的新摘要" });
        let v = apply_skill_result(&conn, &id, "验证真伪", &data).unwrap();
        assert_eq!(v, 2); // 新版本
        let r = results::latest(&conn, &id).unwrap().unwrap();
        assert_eq!(r.summary, "技能生成的新摘要");
        assert_eq!(r.model_used.as_deref(), Some("skill:验证真伪"));
        assert_eq!(r.category, "学习"); // 继承 prior 分类
        assert!(r.category_manual); // manual 标记沿用
        assert_eq!(r.tags, vec!["旧标签".to_string()]); // 继承标签
        // 缺 summary → 不写版本，报 E_LLM_BAD_OUTPUT
        let before = r.version;
        assert!(matches!(
            apply_skill_result(&conn, &id, "验证真伪", &serde_json::json!({ "foo": 1 })),
            Err(AppError::LlmBadOutput(_))
        ));
        assert_eq!(results::latest(&conn, &id).unwrap().unwrap().version, before);
    }
}

