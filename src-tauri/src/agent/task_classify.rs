//! classify（04 §1）：两级分类。枚举非法 / 解析失败 → 兜底 `其他`，degraded。

use serde_json::Value;

use super::output::{char_count, is_category};
use super::{call_json, prompt, LlmClient};

#[derive(Debug, Clone, PartialEq)]
pub struct ClassifyOutcome {
    pub category: String,
    pub subcategory: Option<String>,
    pub degraded: bool,
}

pub fn run(client: &dyn LlmClient, content: &str) -> ClassifyOutcome {
    let data = call_json(client, "classify", &prompt::build_classify(content));
    if let Some(data) = data {
        if let Some(cat) = data.get("category").and_then(Value::as_str) {
            if is_category(cat) {
                return ClassifyOutcome {
                    category: cat.to_string(),
                    subcategory: clean_subcategory(cat, data.get("subcategory")),
                    degraded: false,
                };
            }
        }
    }
    ClassifyOutcome {
        category: "其他".into(),
        subcategory: None,
        degraded: true,
    }
}

/// 二级分类清洗：`其他` 必为 null；非字符串或 >6 字丢弃（04 §1.2 规则）。
fn clean_subcategory(cat: &str, raw: Option<&Value>) -> Option<String> {
    if cat == "其他" {
        return None;
    }
    let s = raw.and_then(Value::as_str)?;
    let s = s.trim();
    if s.is_empty() || char_count(s) > 6 {
        return None;
    }
    Some(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_util::{scripted, scripted_seq};

    #[test]
    fn valid_category_and_sub() {
        let c = scripted(r#"{"ok":true,"data":{"category":"技术","subcategory":"数据库"}}"#);
        let o = run(&c, "sqlite 相关内容");
        assert_eq!((o.category.as_str(), o.subcategory.as_deref(), o.degraded), ("技术", Some("数据库"), false));
    }

    #[test]
    fn illegal_category_falls_back() {
        let c = scripted(r#"{"ok":true,"data":{"category":"玄学","subcategory":"风水"}}"#);
        let o = run(&c, "x");
        assert_eq!((o.category.as_str(), o.subcategory.as_deref(), o.degraded), ("其他", None, true));
    }

    #[test]
    fn other_clears_subcategory() {
        let c = scripted(r#"{"ok":true,"data":{"category":"其他","subcategory":"乱给"}}"#);
        let o = run(&c, "x");
        assert_eq!((o.category.as_str(), o.subcategory.as_deref()), ("其他", None));
        assert!(!o.degraded);
    }

    #[test]
    fn oversized_subcategory_dropped_but_category_kept() {
        let c = scripted(r#"{"ok":true,"data":{"category":"技术","subcategory":"一二三四五六七八"}}"#);
        let o = run(&c, "x");
        assert_eq!((o.category.as_str(), o.subcategory.as_deref(), o.degraded), ("技术", None, false));
    }

    #[test]
    fn malformed_json_retries_then_falls_back() {
        // 首次坏 JSON、重试给好的 → 成功且非 degraded（call_json 内重试一次）
        let c = scripted_seq(vec![
            " totally not json ".into(),
            r#"{"ok":true,"data":{"category":"产品","subcategory":null}}"#.into(),
        ]);
        let o = run(&c, "x");
        assert_eq!((o.category.as_str(), o.degraded), ("产品", false));
    }

    #[test]
    fn persistent_garbage_degrades() {
        let c = scripted("garbage");
        let o = run(&c, "x");
        assert_eq!((o.category.as_str(), o.degraded), ("其他", true));
    }
}
