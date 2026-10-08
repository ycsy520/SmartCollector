//! 搜索 DTO（02 §3）。混合搜索的语义路依赖向量（P6），keyword 路可离线。
use serde::{Deserialize, Serialize};

use super::fragment::FragmentSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Hybrid,
    Keyword,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Matched {
    Keyword,
    Semantic,
    Both,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub text: String,
    #[serde(default)]
    pub mode: Option<SearchMode>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub fragment: FragmentSummary,
    pub score: f64,
    pub matched: Matched,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub items: Vec<SearchHit>,
    /// embedding 失败自动降级为纯 keyword 时为 true（02 §3.1）。
    pub degraded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_mode_lowercase_and_defaults() {
        let q: SearchQuery = serde_json::from_str(r#"{"text":"sqlite"}"#).unwrap();
        assert!(q.mode.is_none());
        assert!(q.limit.is_none());
        let q2: SearchQuery = serde_json::from_str(r#"{"text":"x","mode":"keyword","limit":10}"#).unwrap();
        assert_eq!(q2.mode, Some(SearchMode::Keyword));
    }

    #[test]
    fn hit_serializes_camel() {
        let hit = SearchHit {
            fragment: FragmentSummary {
                id: "a".into(), title: None, excerpt: "e".into(), status: "done".into(),
                category: None, tags: vec![], created_at: "t".into(), layer: "archived".into(),
                media_type: "text".into(), archived_by: None, media_path: None, flags: vec![],
            },
            score: 0.5,
            matched: Matched::Both,
        };
        let j = serde_json::to_string(&hit).unwrap();
        assert!(j.contains("\"matched\":\"both\""), "{j}");
    }
}
