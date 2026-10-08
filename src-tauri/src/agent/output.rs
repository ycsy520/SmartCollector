//! 全局输出协议解析（04 文档 §0）：`{"ok":true,"data":{...}}` / `{"ok":false,...}`。
//! 纯解析、无副作用、绝不 panic：任何非预期输入都归为 None → 由调用方走各任务兜底。

use serde_json::Value;

/// 一级分类封闭枚举（04 §1.1）；不在表内一律视为兜底。
pub const CATEGORIES: [&str; 8] =
    ["技术", "产品", "商业", "学习", "生活", "资讯", "创意", "其他"];

pub fn is_category(s: &str) -> bool {
    CATEGORIES.contains(&s)
}

/// 按 Unicode 字符计数（非字节），用于中日韩字数校验。
pub fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// 解析后的信封。`data` 仅在 ok=true 且为对象时有值。
#[derive(Debug, Clone)]
pub struct Envelope {
    pub ok: bool,
    pub data: Option<Value>,
    pub error_code: Option<String>,
}

impl Envelope {
    /// 容错解析：剥 Markdown 围栏、截取首个 `{`…末个 `}`，serde 解析，校验含 `ok` 布尔。
    /// 失败（非 JSON / 缺 ok / 类型错）返回 None，交由上层重试或兜底。
    pub fn parse(raw: &str) -> Option<Envelope> {
        let json_str = extract_json_object(raw)?;
        let v: Value = serde_json::from_str(json_str).ok()?;
        let obj = v.as_object()?;
        let ok = obj.get("ok").and_then(Value::as_bool)?;
        let data = if ok {
            obj.get("data").filter(|d| d.is_object()).cloned()
        } else {
            None
        };
        let error_code = obj
            .get("error_code")
            .and_then(Value::as_str)
            .map(str::to_string);
        Some(Envelope {
            ok,
            data,
            error_code,
        })
    }

    /// 成功且 data 为对象时返回之；否则 None（ok=false 或结构不符）。
    pub fn data(&self) -> Option<&Value> {
        if self.ok {
            self.data.as_ref()
        } else {
            None
        }
    }
}

/// 从可能夹带解释文字/围栏的响应里抠出 JSON 对象子串。
/// 先取 ``` 围栏内内容（若有），再定位最外层 `{`…`}`。返回 None 表示无候选。
fn extract_json_object(raw: &str) -> Option<&str> {
    let body = strip_fences(raw);
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&body[start..=end])
}

/// 去掉 ```json … ``` 代码围栏，返回围栏内文本；无围栏则返回去首尾空白的原文。
fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // 丢弃围栏语言标记行（```json / ```\n）
        let after_lang = match rest.find('\n') {
            Some(i) => &rest[i + 1..],
            None => rest,
        };
        if let Some(close) = after_lang.rfind("```") {
            return after_lang[..close].trim();
        }
        return after_lang.trim();
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_ok_envelope() {
        let e = Envelope::parse(r#"{"ok":true,"data":{"category":"技术"}}"#).unwrap();
        assert!(e.ok);
        assert_eq!(e.data().unwrap()["category"], "技术");
    }

    #[test]
    fn parses_fenced_envelope_with_prose() {
        let raw = "好的，结果如下：\n```json\n{\"ok\":true,\"data\":{\"tags\":[\"Rust\"]}}\n```\n以上。";
        let e = Envelope::parse(raw).unwrap();
        assert_eq!(e.data().unwrap()["tags"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn ok_false_is_not_data() {
        let e = Envelope::parse(r#"{"ok":false,"error_code":"NO_CONTENT","message":"空"}"#).unwrap();
        assert!(!e.ok);
        assert!(e.data().is_none());
        assert_eq!(e.error_code.as_deref(), Some("NO_CONTENT"));
    }

    #[test]
    fn garbage_and_missing_fields_yield_none() {
        assert!(Envelope::parse("").is_none());
        assert!(Envelope::parse("not json at all").is_none());
        assert!(Envelope::parse(r#"{"data":{}}"#).is_none()); // 缺 ok
        assert!(Envelope::parse(r#"{"ok":"true","data":{}}"#).is_none()); // ok 非布尔
        let e = Envelope::parse(r#"{"ok":true,"data":"str"}"#).unwrap(); // data 非对象
        assert!(e.data().is_none());
    }

    #[test]
    fn brace_extraction_survives_wrapping_text() {
        let e = Envelope::parse(r#"前缀 {"ok":true,"data":{"summary":"x"}} 后缀"#).unwrap();
        assert_eq!(e.data().unwrap()["summary"], "x");
    }
}
