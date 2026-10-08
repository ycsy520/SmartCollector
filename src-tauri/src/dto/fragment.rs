//! 片段相关 DTO（02 §1–2 + §7）。字段与前端 src/types/ipc.ts、02 逐字段一致（camelCase）。
//! 复用 db 层已 camelCase 序列化的 ExtractedLink/ProcessingResult 子集，避免契约漂移。
use serde::{Deserialize, Serialize};

use crate::db::fragments::{EditEntry, Fragment, FragmentListItem};
use crate::db::results::{ExtractedLink, ProcessingResult};

pub use crate::db::skills::Skill;
pub use crate::db::habits::HabitRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FragmentFlagKind {
    Retrash,
    Dup,
    Conflict,
    VerifyFail,
    /// 入队前闸门判定为低价值噪声（02 §1.1 / 03 §2 skipped）。
    Junk,
    /// 本地规则检出隐私信息（身份证/卡号/手机号/口令）：已阻止发送给模型，需用户保密或及时清除。
    /// **判定全程零网络、零 AI**，命中的内容片段本身绝不进标签文案（标签不能变成新的泄露面）。
    Sensitive,
    /// 批次6-①（02 §2.5 / §7.1）：正文在结果生成之后被人工修订，摘要/分类/向量描述的是旧文本。
    /// **读时计算**（`content_updated_at` > 当前结果 `processed_at`），不存布尔位；点「重新处理」即消解。
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentFlag {
    pub kind: FragmentFlagKind,
    pub message: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ref_id: Option<String>,
}

/// 隐私提示标签（02 §7.1）。两条文案分别对应"新收集已被挡住"与"此前已发送模型处理"——
/// 后者是既成事实，不粉饰；用户要知道的是尽快保密或清除，而不是系统假装无事发生。
/// `label` 只写类型词（如"身份证号"），命中的内容片段绝不进文案。
pub fn sensitive_flag(label: &str, blocked: bool) -> FragmentFlag {
    FragmentFlag {
        kind: FragmentFlagKind::Sensitive,
        message: if blocked {
            format!("检测到{label}等隐私信息，已阻止发送给模型")
        } else {
            format!("检测到{label}等隐私信息，此前已发送模型处理")
        },
        action: "请留意保密；确认无需保留可在回收站「彻底删除」即刻清除".into(),
        ref_id: None,
    }
}

/// 「结果已过期」提示标签（02 §7.1，批次6-①）。改过正文后旧摘要/向量与文本不一致，
/// 但不惩罚遗忘：措辞给出路（点重新处理即对齐），不写成错误。走 info 灰（知情）非 warn。
pub fn stale_flag() -> FragmentFlag {
    FragmentFlag {
        kind: FragmentFlagKind::Stale,
        message: "内容已修订 · AI 结果与向量描述的是改前文本".into(),
        action: "点「重新处理」按新正文重整理（会发送给模型）".into(),
        ref_id: None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    pub excerpt: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    pub layer: String,
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archived_by: Option<String>,
    /// 批次21-B（02 §1.4）：图片文件名（相对 media 目录），卡片据此显示缩略图。非图片省略。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub media_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub flags: Vec<FragmentFlag>,
}

impl From<FragmentListItem> for FragmentSummary {
    fn from(i: FragmentListItem) -> Self {
        // 与 `to_summary(build_detail)` 同序：隐私优先于垃圾，stale 独立附加。让 get_fragments
        // 列表摘要也带这些读时旗标（#3-P0），整理模式/卡片在摘要态不再漏显示。
        let mut flags: Vec<FragmentFlag> = Vec::new();
        if let Some(label) = i.sensitive {
            flags.push(sensitive_flag(label, i.status == "skipped"));
        } else if i.junk {
            flags.push(FragmentFlag {
                kind: FragmentFlagKind::Junk,
                message: "系统判定为低价值信息，已跳过 AI 整理".into(),
                action: "仍要保留可放行归档，或点「重新处理」强制整理".into(),
                ref_id: None,
            });
        }
        if i.stale {
            flags.push(stale_flag());
        }
        Self {
            id: i.id,
            title: i.title,
            excerpt: i.excerpt,
            status: i.status,
            category: i.category,
            tags: i.tags,
            created_at: i.created_at,
            layer: i.layer,
            media_type: i.media_type,
            archived_by: i.archived_by,
            media_path: i.media_path,
            flags,
        }
    }
}

/// 02 §2.2 的 result / priorResults 元素（= ProcessingResult 的前端视图子集）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentResult {
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subcategory: Option<String>,
    pub tags: Vec<String>,
    pub summary: String,
    pub links: Vec<ExtractedLink>,
    pub degraded: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model_used: Option<String>,
    pub processed_at: String,
}

impl From<&ProcessingResult> for FragmentResult {
    fn from(r: &ProcessingResult) -> Self {
        Self {
            category: r.category.clone(),
            subcategory: r.subcategory.clone(),
            tags: r.tags.clone(),
            summary: r.summary.clone(),
            links: r.links.clone(),
            degraded: r.degraded,
            model_used: r.model_used.clone(),
            processed_at: r.processed_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ManualFlags {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tags: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentDetail {
    pub id: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub external_url: Option<String>,
    pub source: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<FragmentResult>,
    pub layer: String,
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archived_by: Option<String>,
    pub reviewed: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub trashed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
    /// 批次6-①：人工修订账（正文/笔记的改动），渲染在详情页原文下方。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub edit_log: Vec<EditEntry>,
    /// 批次21-B（02 §1.3）：图片文件名，**相对 `media` 目录**（绝对目录由 `ConfigDTO.mediaDir` 给）。
    /// 非图片为 None。前端据此拼 `convertFileSrc(dir + "/" + name)` 显示缩略图，
    /// 不存绝对路径——整个数据目录搬家后图片仍然找得回来。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub media_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub prior_results: Vec<FragmentResult>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub flags: Vec<FragmentFlag>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub manual: Option<ManualFlags>,
}

/// Fragment → FragmentDetail 基础视图（status/result/manual 等由 command 层补齐后覆盖）。
pub fn detail_base(f: &Fragment) -> FragmentDetail {
    FragmentDetail {
        id: f.id.clone(),
        content: f.content.clone(),
        title: f.title.clone(),
        external_url: f.external_url.clone(),
        source: f.source.clone(),
        status: String::new(),
        created_at: f.created_at.clone(),
        updated_at: f.updated_at.clone(),
        result: None,
        layer: f.layer.clone(),
        media_type: f.media_type.clone(),
        archived_by: f.archived_by.clone(),
        reviewed: f.reviewed,
        trashed_at: f.trashed_at.clone(),
        note: f.note.clone(),
        edit_log: serde_json::from_str(&f.edit_log).unwrap_or_default(),
        media_path: f.media_path.clone(),
        prior_results: vec![],
        flags: vec![],
        manual: None,
    }
}

// ============ 入参 ============

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitInput {
    pub content: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    /// 是否经人工分拣（02 §2.1 补审队列）：`Some(false)` 筛"超时替你收但未补审"的归档条目。
    #[serde(default)]
    pub reviewed: Option<bool>,
    /// 归档来源（`manual` / `auto`），与 `reviewed` 配合定位补审队列。
    #[serde(default)]
    pub archived_by: Option<String>,
}

impl Default for ListQuery {
    fn default() -> Self {
        // 02 §2.1：layer 默认 archived（列表/检索只在归档层）。
        Self {
            offset: None,
            limit: None,
            layer: Some("archived".into()),
            status: None,
            category: None,
            tag: None,
            source: None,
            media_type: None,
            reviewed: None,
            archived_by: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePatch {
    /// 批次6-①（02 §2.5）：人工修订正文。**纯本地更新，绝不外发**；改后重算 hash、刷 FTS、
    /// 删旧向量、置「结果已过期」读时标记、追加 edit_log。撞活跃同文 → `E_STATE_CONFLICT`。
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// 02 §2.2：人工附言。缺省=不改；给定=覆盖（空串清除）。
    #[serde(default)]
    pub note: Option<String>,
}

// ============ 出参包装 ============

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitOutcome {
    pub fragment_id: String,
    pub status: String,
    /// true=原文与既有活跃片段同 hash，未新建，回执指向那一条（幂等收集，02 §1.1）。
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
}

/// `run_skill` 出参（02 §2.7）：新结果版本号。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillOutcome {
    pub new_version: i64,
}

/// `submit_clipboard` 出参（02 §1.2）：两种形状之一，serde 按 untagged 择一序列化。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ClipboardOutcome {
    Collected(SubmitOutcome),
    Skipped { skipped: bool, reason: String },
}

/// `list_related` 单条命中（02 §7.4）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedHit {
    pub fragment: FragmentSummary,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedOutcome {
    pub items: Vec<RelatedHit>,
}

/// `get_week_digest` 出参（02 §7.4）：近 7 天分拣计量 + 一句数据推导导语。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeekDigest {
    pub sorted: i64,
    pub trashed: i64,
    pub auto_archived: i64,
    pub pending: i64,
    pub insight: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_serializes_camelcase_and_omits_empty() {
        let s = FragmentSummary {
            id: "x".into(),
            title: None,
            excerpt: "e".into(),
            status: "done".into(),
            category: Some("技术".into()),
            tags: vec!["Rust".into()],
            created_at: "2026-01-01".into(),
            layer: "archived".into(),
            media_type: "text".into(),
            archived_by: None,
            media_path: None,
            flags: vec![],
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"createdAt\""));
        assert!(j.contains("\"mediaType\""));
        assert!(!j.contains("archivedBy")); // None 省略
        assert!(!j.contains("flags")); // 空省略
        assert!(!j.contains("title")); // None 省略
    }

    #[test]
    fn flag_kind_is_kebab_case() {
        let f = FragmentFlag { kind: FragmentFlagKind::VerifyFail, message: "m".into(), action: "a".into(), ref_id: None };
        let j = serde_json::to_string(&f).unwrap();
        assert!(j.contains("\"verify-fail\""), "{j}");
    }

    #[test]
    fn list_query_defaults_to_archived_layer() {
        // Default 实现在 layer 缺省时给出 archived（02 §2.1）；command 层在整条 query 缺省时使用它。
        assert_eq!(ListQuery::default().layer.as_deref(), Some("archived"));
        // serde 对 `{}` 逐字段 default → layer=None，由 command 层补 archived。
        let q: ListQuery = serde_json::from_str("{}").unwrap();
        assert!(q.layer.is_none());
        // 显式覆盖
        let q2: ListQuery = serde_json::from_str(r#"{"layer":"buffer","limit":50}"#).unwrap();
        assert_eq!(q2.layer.as_deref(), Some("buffer"));
        assert_eq!(q2.limit, Some(50));
    }

    #[test]
    fn result_from_processing_result_maps_fields() {
        let pr = ProcessingResult {
            id: "r1".into(),
            version: 2,
            category: "技术".into(),
            subcategory: Some("数据库".into()),
            category_manual: false,
            tags: vec!["SQLite".into()],
            tags_manual: false,
            summary: "s".into(),
            links: vec![ExtractedLink { url: "https://a".into(), text: None }],
            degraded: true,
            model_used: Some("qwen3-max".into()),
            processed_at: "2026-01-01".into(),
        };
        let fr = FragmentResult::from(&pr);
        assert_eq!(fr.category, "技术");
        assert_eq!(fr.model_used.as_deref(), Some("qwen3-max"));
        let j = serde_json::to_string(&fr).unwrap();
        assert!(j.contains("\"modelUsed\"") && j.contains("\"processedAt\""));
        assert!(!j.contains("\"text\"")); // link.text None 省略
    }
}
