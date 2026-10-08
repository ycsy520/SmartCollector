//! Markdown 导出 command（02 §8）。只做「定范围 + 取详情 + 交给 services::export 落盘」，
//! 渲染与文件命名规则全在 `services/export.rs`（本层不碰字符串拼装）。
//!
//! 范围两档：`ids` 显式给定（单条/多选）优先；否则用与列表同口径的筛选条件取**全量 id**
//! （不分页——导出的范围必须等于用户看到的范围，而不是它的第一页）。
use std::path::Path;

use rusqlite::Connection;
use tauri::{AppHandle, Manager, State};

use crate::db::fragments;
use crate::dto::export as ed;
use crate::error::AppError;
use crate::services::export as render;
use crate::state::AppState;

use super::fragment::build_detail;

/// 解析导出范围 → 有序且去重的片段 id。`layer="all"` 表示不限层。
/// `ids` 为空数组视同"未指定"（前端全量导出不必拼空数组），落到筛选条件。
pub(crate) fn resolve_ids(conn: &Connection, q: &ed::ExportQuery) -> Result<Vec<String>, AppError> {
    if let Some(ids) = q.ids.as_deref().filter(|v| !v.is_empty()) {
        let mut seen = Vec::<String>::new();
        for id in ids {
            if !seen.iter().any(|s| s == id) {
                seen.push(id.clone());
            }
        }
        return Ok(seen);
    }
    let layer = q.layer.as_deref().unwrap_or("archived"); // 与 §2.1 列表默认层一致
    fragments::ids_matching(
        conn,
        &fragments::ListFilter {
            layer: if layer == "all" { None } else { Some(layer) },
            status: q.status.as_deref(),
            category: q.category.as_deref(),
            tag: q.tag.as_deref(),
            source: q.source.as_deref(),
            media_type: q.media_type.as_deref(),
            reviewed: None,    // 导出范围不含补审维度筛选（ExportQuery 未携带，02 §8）
            archived_by: None,
        },
    )
}

/// 导出到调用方给定的目录（目录须已由调用方创建，且路径只由我们生成）。
pub(crate) fn export_fragments_impl(
    conn: &Connection,
    dir: &Path,
    q: &ed::ExportQuery,
) -> Result<ed::ExportOutcome, AppError> {
    let ids = resolve_ids(conn, q)?;
    if ids.is_empty() {
        return Err(AppError::InputEmpty); // 空范围不建空目录假成功
    }
    let mut docs = Vec::with_capacity(ids.len());
    for id in &ids {
        docs.push(build_detail(conn, id)?); // 命中已删除的 id → E_NOT_FOUND，不静默少导
    }
    render::write_all(dir, &docs, q.include_versions.unwrap_or(false))
}

/// 02 §8：把片段导出为 Markdown 目录。目录固定 `app_data_dir/exports/<UTC 时间戳>`——
/// 不弹保存对话框（那要引 `tauri-plugin-dialog`），也不接受前端传路径（防目录穿越）。
#[tauri::command]
pub fn export_fragments(
    state: State<'_, AppState>,
    app: AppHandle,
    query: ed::ExportQuery,
) -> Result<ed::ExportOutcome, AppError> {
    let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("取应用数据目录失败：{e}")))?
        .join("exports")
        .join(ts.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Internal(format!("创建导出目录失败：{e}")))?;
    export_fragments_impl(&conn, &dir, &query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::fragment::SubmitInput;

    fn submit(conn: &Connection, content: &str) -> String {
        let input = SubmitInput {
            content: content.into(),
            title: None,
            note: None,
            skill_ids: vec![],
        };
        let out = super::super::fragment::submit_text_impl(conn, &input).unwrap();
        out.fragment_id
    }

    fn archive(conn: &Connection, id: &str) {
        fragments::set_fragment_layer(conn, id, "archived").unwrap();
    }

    #[test]
    fn ids_scope_wins_over_filters_and_dedups() {
        let conn = crate::db::test_conn();
        let a = submit(&conn, "第一条要导出的内容");
        let b = submit(&conn, "第二条要导出的内容");
        archive(&conn, &a);
        let q = ed::ExportQuery {
            ids: Some(vec![a.clone(), b.clone(), a.clone()]),
            ..Default::default()
        };
        assert_eq!(resolve_ids(&conn, &q).unwrap(), vec![a.clone(), b]);
        // 缓冲区未归档的 b 也在显式 ids 里 → 详情取得到（导出听用户的，不按层二次裁剪）
        let dir = temp_dir();
        let out = export_fragments_impl(&conn, &dir, &q).unwrap();
        assert_eq!(out.files.len(), 3, "两条 + _index.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filter_scope_uses_list_default_layer_and_all_spans_layers() {
        let conn = crate::db::test_conn();
        let buffered = submit(&conn, "还在缓冲区的内容");
        let archived = submit(&conn, "已经归档放行的内容");
        archive(&conn, &archived);
        let dir = temp_dir();

        let by_default = ed::ExportQuery {
            layer: Some("archived".into()),
            ..Default::default()
        };
        let ids = resolve_ids(&conn, &by_default).unwrap();
        assert_eq!(ids, vec![archived.clone()], "默认与列表同口径：只有归档层");

        let everything = ed::ExportQuery {
            ids: Some(vec![]), // 空数组 = 未指定，落到筛选
            layer: Some("all".into()),
            ..Default::default()
        };
        let ids = resolve_ids(&conn, &everything).unwrap();
        assert_eq!(ids.len(), 2, "all 时缓冲区也算进来");
        assert!(ids.contains(&buffered), "未归档的那条也在 all 范围内");

        let out = export_fragments_impl(&conn, &dir, &everything).unwrap();
        assert_eq!(out.files.len(), 3);
        assert!(std::fs::read_to_string(dir.join("_index.md"))
            .unwrap()
            .contains(&format!("({})", out.files[0])));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_scope_and_unknown_id_are_errors_not_empty_success() {
        let conn = crate::db::test_conn();
        let dir = temp_dir();
        let q = ed::ExportQuery {
            category: Some("不存在".into()),
            layer: Some("all".into()),
            ..Default::default()
        };
        assert!(matches!(
            export_fragments_impl(&conn, &dir, &q),
            Err(AppError::InputEmpty)
        ), "空范围不假装成功");
        let q2 = ed::ExportQuery {
            ids: Some(vec!["no-such-id".into()]),
            ..Default::default()
        };
        assert!(matches!(
            export_fragments_impl(&conn, &dir, &q2),
            Err(AppError::NotFound)
        ), "id 打错要报错，别悄悄导出一条别的");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("sc-export-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
