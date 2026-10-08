//! 图片录入 command（02 §1.4 `submit_image`，批次21-B）。
//!
//! 与文本录入的分野：**只存不认**。字节由前端从粘贴事件取出（超过 5 MB 已在 canvas 里压过一道），
//! 后端只做写路径该做的校验——解 base64、按魔数认格式、限尺寸、落文件、落一行占位片段。
//! 占位片段走 `insert_skipped`，worker 领不到 → 零 token；图片本身永不进 FTS 之外的检索语义
//! （可检索面只有那行占位标记与人工附言）。
use rusqlite::Connection;
use serde::Deserialize;
use std::path::Path;
use tauri::{AppHandle, Emitter, State};

use crate::db::fragments;
use crate::dto::fragment as fd;
use crate::error::AppError;
use crate::events::{FragmentCreated, FRAGMENT_CREATED};
use crate::services::media;
use crate::state::AppState;

/// 02 §1.4 入参。`data` = 纯 base64（不含 `data:` 前缀，前端已剥）；`note` = 人工描述，
/// 是唯一能让这张图日后被查到的东西。
/// `content`（批次23）= 用户在输入框里与图**同时**写的正文：给了就和图合成**一条**，
/// 不给就还是批次21-B 那行只含占位文件名的图片条目。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    pub data: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

/// 落一张图片片段。顺序刻意是**先文件后库行**：反过来（先库后文件）一旦写文件失败，
/// 库里就留下一行指向不存在文件的片段——列表里一个永远加载不出来的破图，且没人知道为什么。
/// 现在最坏情况是目录里多一个没人引用的文件（同名重写或日后手工清掉即可），界面不会说谎。
pub(crate) fn submit_image_impl(
    conn: &Connection,
    dir: &Path,
    input: &ingest::Checked,
) -> Result<fd::SubmitOutcome, AppError> {
    let name = input.file_name.clone();
    media::write(dir, &name, &input.bytes)?;
    // 占位行 `【图片】<文件名>` 在**有正文时也必须留着**，它不只是给眼睛看的：活跃去重唯一索引
    // `uq_frags_active_hash` 按正文 sha256 生效，去掉它就变成"同一段文字配两张不同的图"会撞索引
    // （或被判 duplicate 而把刚写的文件回滚掉）。文件名含本行 id，故天然唯一。
    let marker = format!("【图片】{name}");
    let (content, media_type) = match input.content.as_deref() {
        Some(text) => (format!("{text}\n\n{marker}"), "text"),
        None => (marker.clone(), "image"),
    };
    let new = fragments::NewFragment {
        content: &content,
        title: None,
        source: "manual",
        external_url: None,
        media_type: Some(media_type),
        note: input.note.as_deref(),
        media_path: Some(&name),
        id: Some(&input.fragment_id),
    };
    // 带正文的那条走**与 submit_text 同一条闸门**（含本地隐私检测）：文字是用户亲手写的，
    // 该被整理就该被整理；图片字节仍然只在磁盘上，一个字符都不进正文，也就永不外发。
    // 纯图片那条维持批次21-B：一律 skipped，占位正文发给模型只会换来凭空编造的摘要。
    let gate = input.content.is_some() && fragments::assess_skip(&content, false).is_none();
    let landed = if gate {
        fragments::insert(conn, &new)
    } else {
        fragments::insert_skipped(conn, &new)
    };
    match landed {
        Ok(f) => Ok(fd::SubmitOutcome {
            fragment_id: f.id,
            status: if gate { "pending".into() } else { "skipped".into() },
            duplicate: false,
        }),
        Err(e) => {
            // 库行没落成 → 刚才那个文件没有人引用，顺手带走（不留孤儿）。
            media::delete(dir, &name);
            Err(e)
        }
    }
}

/// 解码 + 认格式 + 限尺寸，产出"可以落盘的一张图"。放在独立模块里以便脱离 Tauri 单测。
pub(crate) mod ingest {
    use super::*;

    /// 已通过校验的一张图：id 与文件名已定，只差落盘。
    pub struct Checked {
        pub fragment_id: String,
        pub file_name: String,
        pub bytes: Vec<u8>,
        pub note: Option<String>,
        /// 与图同时提交的正文（已 trim，空串归一成 None）。
        pub content: Option<String>,
    }

    /// 把可选文本归一成"有内容才 Some"——三处（note / content / 前端草稿）同一条规则，
    /// 免得一串空格也能把一条纯图片条目伪装成"带正文"的那类。
    fn clean(text: Option<&str>) -> Option<String> {
        text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
    }

    /// base64 → 字节 → 魔数定后缀 → 预生成 id 与文件名（文件名含 id，故必须先定 id）。
    pub fn check(input: &ImageInput) -> Result<Checked, AppError> {
        let bytes = media::decode_base64(&input.data)?;
        if bytes.len() as u64 > media::MAX_IMAGE_BYTES {
            return Err(AppError::InputTooLarge);
        }
        // 只认魔数，不认前端声明的 MIME/后缀：声明与实际不符才是常态，
        // 而后缀会被 <img> 与导出的 Markdown 直接消费（放个 .svg 进来就是自埋脚本）。
        let ext = media::sniff_ext(&bytes).ok_or_else(|| {
            AppError::InputInvalid("只支持 PNG / JPEG / WebP / GIF 图片".into())
        })?;
        let fragment_id = uuid::Uuid::new_v4().to_string();
        let file_name = format!("{fragment_id}.{ext}");
        Ok(Checked {
            fragment_id,
            file_name,
            bytes,
            note: clean(input.note.as_deref()),
            content: clean(input.content.as_deref()),
        })
    }
}

/// 02 §1.4：收藏一张图片（原样存文件 + 一行不经模型的占位片段）。
#[tauri::command]
pub fn submit_image(
    state: State<'_, AppState>,
    app: AppHandle,
    input: ImageInput,
) -> Result<fd::SubmitOutcome, AppError> {
    let checked = ingest::check(&input)?;
    let dir = media::media_dir()?;
    let outcome = {
        let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        submit_image_impl(&conn, &dir, &checked)?
    };
    let _ = app.emit(
        FRAGMENT_CREATED,
        FragmentCreated { fragment_id: outcome.fragment_id.clone(), source: "manual" },
    );
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1×1 像素 PNG（标准 base64）。
    const PNG_1PX: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn tmp_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sc-cmd-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn check(data: &str, note: Option<&str>) -> ingest::Checked {
        check_with(data, note, None)
    }

    /// 批次23：带正文的那条路径（图 + 文字合成一条）。
    fn check_with(data: &str, note: Option<&str>, content: Option<&str>) -> ingest::Checked {
        ingest::check(&ImageInput {
            data: data.to_string(),
            note: note.map(str::to_string),
            content: content.map(str::to_string),
        })
        .unwrap()
    }

    #[test]
    fn collects_image_as_skipped_row_with_file() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let c = check(PNG_1PX, Some("  奏折上的朱批  "));
        let out = submit_image_impl(&conn, &dir, &c).unwrap();
        assert_eq!(out.status, "skipped");
        assert!(!out.duplicate);

        let d = crate::commands::fragment::get_fragment_impl(&conn, &out.fragment_id).unwrap();
        assert_eq!(d.media_type, "image");
        assert_eq!(d.status, "skipped");
        assert_eq!(d.note.as_deref(), Some("奏折上的朱批")); // trim + 落库
        // 库里只存相对文件名，且文件名 = 片段 id + 魔数判出的后缀
        assert_eq!(d.media_path.as_deref(), Some(c.file_name.as_str()));
        assert!(d.content.contains(&c.file_name));
        // 文件真在磁盘上，字节与解码结果一致
        let on_disk = std::fs::read(dir.join(&c.file_name)).unwrap();
        assert_eq!(on_disk, c.bytes);
        assert_eq!(media::dir_usage(&dir), (c.bytes.len() as u64, 1));
        // worker 领不到（零 token）
        assert_eq!(
            crate::db::jobs::status_of(&conn, &out.fragment_id).unwrap(),
            Some(crate::db::jobs::Status::Skipped)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_note_becomes_none_and_is_not_stored_as_blank() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let c = check(PNG_1PX, Some("   "));
        assert!(c.note.is_none());
        let out = submit_image_impl(&conn, &dir, &c).unwrap();
        let d = crate::commands::fragment::get_fragment_impl(&conn, &out.fragment_id).unwrap();
        assert!(d.note.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_image_bytes_and_oversize_and_garbage_base64() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        // 合法 base64，但不是图片（这里是一行 HTML 的字节）→ 魔数不认
        let not_image = "PGltZyBzcmM9eCBvbmxvYWQ9YWxlcnQoMSk+";
        assert!(matches!(
            ingest::check(&ImageInput { data: not_image.into(), note: None, content: None }),
            Err(AppError::InputInvalid(_))
        ));
        // 空/非法 base64
        assert!(ingest::check(&ImageInput { data: String::new(), note: None, content: None }).is_err());
        // 真落盘前就被拒 → 目录保持干净
        assert_eq!(media::dir_usage(&dir), (0, 0));
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM fragments", [], |r| r.get(0))
                .unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_row_write_leaves_no_orphan_file() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let c = check(PNG_1PX, None);
        // 制造库写失败：同 id 先占一行（主键冲突）→ insert_skipped 报错
        conn.execute(
            "INSERT INTO fragments (id, content, source, content_hash, char_count, created_at, updated_at)
             VALUES (?1,'x','manual','h',1,'2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
            rusqlite::params![c.fragment_id],
        )
        .unwrap();
        let err = submit_image_impl(&conn, &dir, &c).unwrap_err();
        assert!(matches!(err, AppError::StateConflict), "主键冲突应是写冲突，实际: {err:?}");
        assert!(!dir.join(&c.file_name).exists(), "库行没落成就不该留下文件");
        assert_eq!(media::dir_usage(&dir), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_identical_images_are_two_rows_not_a_duplicate() {
        // 图片没有可比的内容 hash（正文只是占位），故同一张图粘两次得两条记录——
        // 与文本的幂等收集刻意不同，不假装去过重。
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let a = submit_image_impl(&conn, &dir, &check(PNG_1PX, None)).unwrap();
        let b = submit_image_impl(&conn, &dir, &check(PNG_1PX, None)).unwrap();
        assert_ne!(a.fragment_id, b.fragment_id);
        assert!(!b.duplicate);
        assert_eq!(media::dir_usage(&dir).1, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 批次23：图 + 文字 = **一条**碎片，文字照常进管线，图片字节仍一个字符都不进正文。
    #[test]
    fn text_with_image_lands_one_queued_row() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let c = check_with(PNG_1PX, Some("附言"), Some("测试图片情况"));
        let out = submit_image_impl(&conn, &dir, &c).unwrap();
        assert_eq!(out.status, "pending", "带正文的那条该被整理，不该因为附带一张图就永不加工");

        let d = crate::commands::fragment::get_fragment_impl(&conn, &out.fragment_id).unwrap();
        assert_eq!(d.media_type, "text", "有真实正文的行按文本口径参与检索与再加工");
        assert_eq!(d.media_path.as_deref(), Some(c.file_name.as_str()), "图仍在这条自己身上");
        assert_eq!(d.content, format!("测试图片情况\n\n【图片】{}", c.file_name));
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM fragments", [], |r| r.get(0))
                .unwrap(),
            1,
            "一次收集只落一行——分成两条是本批要修的缺陷"
        );
        assert_eq!(
            crate::db::jobs::status_of(&conn, &out.fragment_id).unwrap(),
            Some(crate::db::jobs::Status::Pending)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 占位行为什么必须留着：活跃去重索引按正文 sha256 生效。
    #[test]
    fn same_text_with_two_images_does_not_collide() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let a = submit_image_impl(&conn, &dir, &check_with(PNG_1PX, None, Some("同一句说明"))).unwrap();
        let b = submit_image_impl(&conn, &dir, &check_with(PNG_1PX, None, Some("同一句说明"))).unwrap();
        assert_ne!(a.fragment_id, b.fragment_id);
        assert_eq!(media::dir_usage(&dir).1, 2);
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM fragments", [], |r| r.get(0))
                .unwrap(),
            2,
            "同一段文字配两张不同的图是两条真实收藏，不该撞唯一索引"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 合并行不绕闸门：命中本地隐私检测就照旧跳过，不外发用户写的字。
    #[test]
    fn merged_row_still_hits_the_privacy_gate() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let c = check_with(PNG_1PX, None, Some("备用机 13800138000，密码：Xk9#tR2mLq"));
        let out = submit_image_impl(&conn, &dir, &c).unwrap();
        assert_eq!(out.status, "skipped", "隐私闸门对带图的合并行同样生效");
        let d = crate::commands::fragment::get_fragment_impl(&conn, &out.fragment_id).unwrap();
        assert!(d.media_path.is_some(), "图照样原样收藏，只是这条不外出");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 只给空白正文 = 没给：维持批次21-B 的纯图片行为（占位正文 + skipped + image 类型）。
    #[test]
    fn blank_content_falls_back_to_pure_image_row() {
        let conn = crate::db::test_conn();
        let dir = tmp_dir();
        let c = check_with(PNG_1PX, None, Some("   \n "));
        assert!(c.content.is_none(), "一串空白不该把图片条目伪装成带正文的那类");
        let out = submit_image_impl(&conn, &dir, &c).unwrap();
        assert_eq!(out.status, "skipped");
        let d = crate::commands::fragment::get_fragment_impl(&conn, &out.fragment_id).unwrap();
        assert_eq!(d.media_type, "image");
        assert_eq!(d.content, format!("【图片】{}", c.file_name));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
