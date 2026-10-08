//! extract_links（04 §4）：**正则（此处手写扫描，避免引入 regex 依赖）必跑**，确定性抽取
//! http(s)/www URL 并规范化、去重、截断；LLM 仅补锚文本 + 找缺协议隐性链接。
//! 此任务的兜底就是扫描结果本身——**永不 degraded、永不因 LLM 失败而清空**。

use serde_json::{json, Value};

use crate::db::results::ExtractedLink;
use super::{call_json, prompt, LlmClient};

const MAX_LINKS: usize = 20;
const MAX_EXTRA: usize = 5;

/// URL 结束符：空白、尖括号/引号、常见中英文闭合标点、CJK 句读。
fn is_terminator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '<' | '>' | '"' | '\'' | ')' | ']' | '}' | '（' | '）' | '【' | '】' | '。' | '，'
                | '、' | '；' | '：' | '！' | '？'
        )
}

pub fn run(client: &dyn LlmClient, content: &str) -> Vec<ExtractedLink> {
    let mut links = scan_links(content);
    if links.is_empty() {
        return links; // 无 http/www 命中：不调用 LLM（省一次调用；缺协议裸域留待真实 LLM 阶段）
    }
    let urls_json =
        json!(links.iter().map(|l| l.url.as_str()).collect::<Vec<_>>()).to_string();
    if let Some(data) = call_json(client, "extract_links", &prompt::build_extract_links(&urls_json, content)) {
        enrich_texts(&mut links, &data);
        append_extra(&mut links, &data);
    }
    links.truncate(MAX_LINKS);
    links
}

/// 扫描并规范化 URL：http(s):// 原样；www. 前缀补 https://。去重保序、裁尾标点。
pub fn scan_links(content: &str) -> Vec<ExtractedLink> {
    let chars: Vec<char> = content.chars().collect();
    let mut out: Vec<ExtractedLink> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let needs_https = if is_http_at(&chars, i) {
            false
        } else if is_www_at(&chars, i) {
            true
        } else {
            i += 1;
            continue;
        };
        let start = i;
        while i < chars.len() && !is_terminator(chars[i]) {
            i += 1;
        }
        let raw: String = chars[start..i].iter().collect();
        let trimmed = trim_trailing_punct(&raw);
        if trimmed.is_empty() {
            continue;
        }
        let url = normalize(trimmed, needs_https);
        if is_http_url(&url) && !out.iter().any(|l| l.url == url) {
            out.push(ExtractedLink {
                url: url.to_string(),
                text: None, // 锚文本交 LLM 补（04 §4.2）
            });
            if out.len() >= MAX_LINKS {
                break;
            }
        }
    }
    out
}

/// 用 LLM 返回的 links[] 给扫描到的 URL 补锚文本：仅按 url 精确匹配、text 限长，不改/增 URL。
fn enrich_texts(links: &mut [ExtractedLink], data: &Value) {
    let Some(arr) = data.get("links").and_then(Value::as_array) else {
        return;
    };
    for item in arr {
        let Some(url) = item.get("url").and_then(Value::as_str) else {
            continue;
        };
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.chars().count() <= 20);
        if let Some(t) = text {
            if let Some(slot) = links.iter_mut().find(|l| l.url == url && l.text.is_none()) {
                slot.text = Some(t.to_string());
            }
        }
    }
}

/// 追加 LLM 在 extra 里补全的缺协议链接：规范化后仍须是 http(s)，去重、上限 5。
fn append_extra(links: &mut Vec<ExtractedLink>, data: &Value) {
    let Some(arr) = data.get("extra").and_then(Value::as_array) else {
        return;
    };
    let mut added = 0usize;
    for item in arr {
        if added >= MAX_EXTRA {
            break;
        }
        let Some(raw) = item.as_str().or_else(|| item.get("url").and_then(Value::as_str)) else {
            continue;
        };
        let url = normalize(raw.trim(), !raw.starts_with("http"));
        if is_http_url(&url) && !links.iter().any(|l| l.url == url) {
            links.push(ExtractedLink {
                url: url.to_string(),
                text: None,
            });
            added += 1;
        }
    }
}

fn is_http_at(chars: &[char], i: usize) -> bool {
    let w = |k: usize| chars.get(i + k).map(|c| c.to_ascii_lowercase()).unwrap_or('\0');
    w(0) == 'h' && w(1) == 't' && w(2) == 't' && w(3) == 'p'
}

fn is_www_at(chars: &[char], i: usize) -> bool {
    let w = |k: usize| chars.get(i + k).map(|c| c.to_ascii_lowercase()).unwrap_or('\0');
    w(0) == 'w' && w(1) == 'w' && w(2) == 'w' && w(3) == '.'
}

fn trim_trailing_punct(s: &str) -> &str {
    let mut end = s.len();
    while end > 0 {
        let last = s[..end].chars().next_back().unwrap();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?') {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    &s[..end]
}

fn normalize(url: &str, needs_https: bool) -> String {
    if needs_https && !url.starts_with("http") {
        format!("https://{url}")
    } else {
        url.to_string()
    }
}

/// 安全闸：只接受 http(s):// 且 scheme 后有实际内容的链接，挡掉 javascript:/data:/file:
/// 以及裸 `http://`（无 host）（与前端 url.ts 同义）。
fn is_http_url(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    match l.strip_prefix("https://").or_else(|| l.strip_prefix("http://")) {
        Some(rest) => !rest.is_empty(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_util::{scripted, scripted_seq};

    #[test]
    fn scans_http_urls_dedups_and_caps() {
        let c = scripted("bad"); // 令 LLM 失败，仅验证扫描兜底
        let content = "看 https://a.com/x 和 https://a.com/x 再来 https://b.org/y";
        let links = run(&c, content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://a.com/x");
    }

    #[test]
    fn image_urls_appended_at_tail_are_scanned() {
        // 前端富文本粘贴会把 <img> 地址接到正文末尾（各占一行）——这里锁住"接得住"：
        // 换行是终止符，故每个地址各成一条；扩展名/查询串都不该被裁掉。
        let content = "这段是网页正文。\n\nhttps://cdn.example.com/img/vector-arch.png\nhttps://cdn.example.com/img/chart.svg?w=800";
        let links = scan_links(content);
        assert_eq!(
            links.iter().map(|l| l.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://cdn.example.com/img/vector-arch.png",
                "https://cdn.example.com/img/chart.svg?w=800"
            ]
        );
    }

    #[test]
    fn www_gets_https_prefix() {        let links = scan_links("参考 www.example.com/page 结束");
        assert_eq!(links[0].url, "https://www.example.com/page");
    }

    #[test]
    fn trailing_punctuation_trimmed() {
        let links = scan_links("见 https://ex.com/a. 与 https://ex.com/b，");
        assert_eq!(links[0].url, "https://ex.com/a");
    }

    #[test]
    fn rejects_non_http_schemes() {
        // XSS/伪协议/data 一律不得成链
        let content = "<img src=x onerror=alert(document.cookie)> javascript:alert(1) data:image/png;base64,AAA file:///etc/passwd";
        assert!(scan_links(content).is_empty());
    }

    #[test]
    fn llm_enriches_anchor_text_and_extra() {
        let c = scripted_seq(vec![
            r#"{"ok":true,"data":{"links":[{"url":"https://ex.com/a","text":"示例甲"}],"extra":["blog.other.com/post"]}}"#
                .into(),
        ]);
        let links = run(&c, "详见 https://ex.com/a");
        assert_eq!(links[0].text.as_deref(), Some("示例甲"));
        assert!(links.iter().any(|l| l.url == "https://blog.other.com/post"));
    }

    #[test]
    fn never_degraded_when_llm_fails() {
        let c = scripted_seq(vec!["garbage".into(), "garbage".into()]);
        let links = run(&c, "有 https://keep.me 这条");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://keep.me"); // 兜底=扫描结果本身
    }
}
