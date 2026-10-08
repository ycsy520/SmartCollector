//! 剪贴板接入（02 §1.2）：**只在用户主动触发时**读一次剪贴板并收进缓冲区。
//!
//! 2026-09-26 用户裁定：不做后台轮询监听——被动把复制过的内容收进库里行为不可信，
//! 收集必须由用户明示动作（输入框粘贴走 `submit_text`，或托盘「收集剪贴板」/前端按钮走
//! `submit_clipboard`）。因此本模块只剩共享的纯落库函数 `collect_text`，无常驻线程、
//! 无 `clip.watch` 配置。

use rusqlite::Connection;

use crate::db::fragments;
use crate::error::AppError;

/// 收集一条剪贴板文本到缓冲区。空/纯空白、或库里已有同内容活跃片段 → 跳过返回 None。
/// 否则以 `source="clipboard"` 落库，纯链接正文记 `media_type="link"`。与 `insert` 同走
/// 事务（主表+队列+FTS 原子）。命中入队前闸门（回捕本 App 界面文案/纯符号）落 `skipped`，
/// 不进流水线（零 token）。纯逻辑、脱离 Tauri 可测。
pub fn collect_text(conn: &Connection, text: &str) -> Result<Option<String>, AppError> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let hash = fragments::hash_of(text);
    if fragments::find_active_by_hash(conn, &hash)?.is_some() {
        return Ok(None); // 库里已有活跃重复
    }
    let trimmed = text.trim();
    let is_link = (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && !trimmed.chars().any(char::is_whitespace);
    let new = fragments::NewFragment {
        content: text,
        title: None,
        source: "clipboard",
        external_url: if is_link { Some(trimmed) } else { None },
        media_type: Some(if is_link { "link" } else { "text" }),
        note: None,
        media_path: None,
        id: None,
    };
    let f = if fragments::assess_skip(text, false).is_some() {
        fragments::insert_skipped(conn, &new)?
    } else {
        fragments::insert(conn, &new)?
    };
    Ok(Some(f.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_empty_and_active_duplicate() {
        let conn = crate::db::test_conn();
        assert!(collect_text(&conn, "   ").unwrap().is_none());
        let id = collect_text(&conn, "剪贴板收集的正文").unwrap().expect("应收集");
        assert!(collect_text(&conn, "剪贴板收集的正文").unwrap().is_none()); // 库内活跃重复
        let d = crate::commands::fragment::get_fragment_impl(&conn, &id).unwrap();
        assert_eq!(d.source, "clipboard");
        assert_eq!(d.layer, "buffer");
        assert_eq!(d.media_type, "text");
    }

    #[test]
    fn recaptured_ui_copy_lands_skipped() {
        let conn = crate::db::test_conn();
        // 回捕本 App 界面文案：应收集但落 skipped，绝不进流水线（零 token）
        let id = collect_text(&conn, "没有待分拣的条目").unwrap().expect("应收集");
        assert_eq!(
            crate::db::jobs::status_of(&conn, &id).unwrap(),
            Some(crate::db::jobs::Status::Skipped)
        );
        // 正常正文照旧进 pending
        let ok = collect_text(&conn, "空印案牵连数千人，朕欲整饬吏治。").unwrap().unwrap();
        assert_eq!(
            crate::db::jobs::status_of(&conn, &ok).unwrap(),
            Some(crate::db::jobs::Status::Pending)
        );
    }

    #[test]
    fn deleted_clipboard_text_can_be_collected_again() {
        let conn = crate::db::test_conn();
        let text = "复制后又被删掉的内容";
        let id = collect_text(&conn, text).unwrap().unwrap();
        fragments::soft_delete(&conn, &id).unwrap(); // 删除释放库内 hash 槽
        // 无 seen 环也不自动重收：只有用户再次明示收集才会落库（此处即用户明示）
        assert!(collect_text(&conn, text).unwrap().is_some());
    }

    #[test]
    fn bare_url_becomes_link_others_text() {
        let conn = crate::db::test_conn();
        let link = collect_text(&conn, "https://example.com/a").unwrap().unwrap();
        let d = crate::commands::fragment::get_fragment_impl(&conn, &link).unwrap();
        assert_eq!(d.media_type, "link");
        assert_eq!(d.external_url.as_deref(), Some("https://example.com/a"));
        // 含空格的串按普通文本收
        let text = collect_text(&conn, "看看 https://example.com/a 这个链接").unwrap().unwrap();
        assert_eq!(
            crate::commands::fragment::get_fragment_impl(&conn, &text).unwrap().media_type,
            "text"
        );
    }
}
