//! tag（04 §2）：3–5 个名词短语。清洗违规项后 <2 个 → 整体兜底 `[]`，degraded。

use super::output::{char_count, is_category};
use super::{call_json, prompt, LlmClient};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct TagOutcome {
    pub tags: Vec<String>,
    pub degraded: bool,
}

pub fn run(client: &dyn LlmClient, content: &str) -> TagOutcome {
    let fallback = || TagOutcome {
        tags: Vec::new(),
        degraded: true,
    };
    let Some(data) = call_json(client, "tag", &prompt::build_tag(content)) else {
        return fallback();
    };
    let Some(arr) = data.get("tags").and_then(Value::as_array) else {
        return fallback();
    };
    let cleaned = clean_tags(arr);
    if cleaned.len() < 2 {
        return fallback(); // 过滤后不足下限：整体视为兜底（04 §2.3）
    }
    TagOutcome {
        tags: cleaned,
        degraded: false,
    }
}

/// 逐项校验 + 去重（保序）+ 截断前 5。
/// 只做**可确定性判定**的粗过滤（长度、控制字符、句子标点、分类词、重复）；
/// 名词性/情绪词等语义判断交由模型与 prompt（04 §2.1），此处不做不可靠的中文分词猜测。
fn clean_tags(arr: &[Value]) -> Vec<String> {
    const SENTENCE_PUNCT: [char; 8] = ['。', '！', '？', '，', '、', '；', '.', '?'];
    let mut seen: Vec<String> = Vec::new();
    for v in arr {
        let Some(raw) = v.as_str() else { continue };
        let t = raw.trim();
        let n = char_count(t);
        if n < 2 || n > 8 {
            continue;
        }
        if is_category(t) {
            continue; // 一级分类词不作标签
        }
        if t.chars().any(|c| c.is_control()) || t.chars().any(|c| SENTENCE_PUNCT.contains(&c)) {
            continue;
        }
        if !seen.iter().any(|s| s == t) {
            seen.push(t.to_string());
        }
        if seen.len() == 5 {
            break;
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_util::{scripted, scripted_seq};

    #[test]
    fn valid_tags_kept() {
        let c = scripted(r#"{"ok":true,"data":{"tags":["Rust","SQLite","性能优化"]}}"#);
        let o = run(&c, "x");
        assert_eq!(o.tags, vec!["Rust", "SQLite", "性能优化"]);
        assert!(!o.degraded);
    }

    #[test]
    fn filters_category_words_punctuation_overlong_and_dedups() {
        let c = scripted(
            r#"{"ok":true,"data":{"tags":["技术","我觉得很有用。","Rust","Rust","学习","K8s"]}}"#,
        );
        let o = run(&c, "x");
        // "技术"/"学习"=分类词丢；"我觉得很有用。"含句读丢；重复 Rust 收敛
        assert_eq!(o.tags, vec!["Rust", "K8s"]);
        assert!(!o.degraded);
    }

    #[test]
    fn caps_at_five() {
        let c = scripted(
            r#"{"ok":true,"data":{"tags":["标签一","标签二","标签三","标签四","标签五","标签六"]}}"#,
        );
        let o = run(&c, "x");
        assert_eq!(o.tags.len(), 5);
    }

    #[test]
    fn below_two_after_filter_is_fallback() {
        let c = scripted(r#"{"ok":true,"data":{"tags":["技术","好"]}}"#); // 分类词 + 单字 全过滤
        let o = run(&c, "x");
        assert!(o.tags.is_empty() && o.degraded);
    }

    #[test]
    fn malformed_falls_back_after_retry() {
        let c = scripted_seq(vec!["{bad".into(), "also bad".into()]);
        let o = run(&c, "x");
        assert!(o.tags.is_empty() && o.degraded);
    }
}
