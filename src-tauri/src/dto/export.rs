//! 导出 DTO（02 §8）。只读聚合：把已入库的片段落成 Markdown 文件。
use serde::{Deserialize, Serialize};

/// 范围两档：给了 `ids` 就只导这些（单条/多选）；否则按筛选条件取全量（不含分页上限）。
/// 字段语义与 `ListQuery`（§2.1）一致，但**无 offset/limit**——导出不该被分页口径截断。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportQuery {
    pub ids: Option<Vec<String>>,
    /// 默认 `archived`（与列表默认层一致）；传 `"all"` 表示不限层。
    pub layer: Option<String>,
    pub status: Option<String>,
    pub category: Option<String>,
    pub tag: Option<String>,
    pub source: Option<String>,
    pub media_type: Option<String>,
    /// 是否把技能再加工的历史版本一起写进 `## 历史版本`。默认 false（只导当前值）。
    pub include_versions: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    /// 本次导出目录的绝对路径（`app_data_dir/exports/<UTC 时间戳>`）。
    pub dir: String,
    /// 已写出的文件名（相对 `dir`），顺序与片段列表一致；`_index.md` 在末尾。
    pub files: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_fields_optional_and_camel_case() {
        let q: ExportQuery = serde_json::from_str("{}").unwrap();
        assert!(q.ids.is_none() && q.layer.is_none());
        let q2: ExportQuery =
            serde_json::from_str(r#"{"ids":["a"],"layer":"all","includeVersions":true,"mediaType":"link"}"#)
                .unwrap();
        assert_eq!(q2.ids.clone().unwrap_or_default(), vec!["a".to_string()]);
        assert_eq!(q2.layer.as_deref(), Some("all"));
        assert_eq!(q2.include_versions, Some(true));
        assert_eq!(q2.media_type.as_deref(), Some("link"));
    }
}
