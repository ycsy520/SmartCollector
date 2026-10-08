//! summarize（04 §3）：短内容直接不生成摘要（见 `MIN_SUMMARIZE_CHARS`）；其余按原文长度分档；
//! 模型失败走抽取式兜底（前 2 句 ≤120 字）degraded；
//! 超字数不判失败，入库前按句读截断到上限。

use super::output::char_count;
use super::{call_json, prompt, LlmClient};

#[derive(Debug, Clone, PartialEq)]
pub struct SummaryOutcome {
    pub summary: String,
    pub degraded: bool,
}

/// 短于此字数的内容**不请求也不产出摘要**（04 §3.1）：短文的"摘要"必然退化成语义复读或被截断，
/// 而诗、词句、路径、短语正是这一档——它们要的是点评/补全（技能），不是压缩。
/// 取 100 也顺带让"≤ 原文 60%"这条防复读夹子恒不生效（50 字档上限 / 0.6 ≈ 83 字 < 100），
/// 故不再加第二个魔数。用户仍可用技能对短内容做加工（技能输出同样落在 summary 列）。
pub const MIN_SUMMARIZE_CHARS: usize = 100;

/// 各长度档位的摘要字数上限（04 §3.1）。
fn cap_for(content_chars: usize) -> usize {
    if content_chars < 300 {
        50
    } else if content_chars <= 3000 {
        150
    } else {
        250
    }
}

pub fn run(client: &dyn LlmClient, content: &str) -> SummaryOutcome {
    let chars = char_count(content);
    if chars < MIN_SUMMARIZE_CHARS {
        return SummaryOutcome {
            summary: String::new(),
            degraded: false,
        };
    }
    let cap = cap_for(chars);
    let model_summary = call_json(client, "summarize", &prompt::build_summarize(content))
        .and_then(|d| {
            d.get("summary")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty());

    match model_summary {
        // 超字数属正常偏差：截断即可，不算兜底（04 §3.3 规则 2）。
        Some(s) if char_count(&s) > cap => SummaryOutcome {
            summary: truncate_sentences(&s, cap),
            degraded: false,
        },
        Some(s) => SummaryOutcome {
            summary: s,
            degraded: false,
        },
        // 模型失败/空 → 抽取式兜底（规则 1：前 2 句，≤120 字）。
        None => SummaryOutcome {
            summary: extractive(content, 120),
            degraded: true,
        },
    }
}

/// 抽取式兜底：跳过 URL，按句读取正文前若干句，截到 max 字。
fn extractive(content: &str, max: usize) -> String {
    let clean = strip_urls(content);
    let mut out = String::new();
    let mut sentences = 0;
    for sentence in split_sentences(&clean) {
        if sentences >= 2 {
            break;
        }
        let piece = sentence.trim();
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('。');
        }
        out.push_str(piece);
        sentences += 1;
        if char_count(&out) >= max {
            break;
        }
    }
    truncate_chars(&out, max)
}

/// 移除 http(s) URL，避免兜底摘要里塞进裸链接（04 §3.1：URL 不必保留）。
/// URL = `http`… 起、到下一个空白或中文句读为止。
fn strip_urls(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if is_http_at(&chars, i) {
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !matches!(chars[i], '。' | '，' | '；' | '、' | '）' | ']' | '】' | ')')
            {
                i += 1;
            }
            continue; // 丢弃整段 URL
        }
        out.push(chars[i]);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_http_at(chars: &[char], i: usize) -> bool {
    let w = |k: usize| chars.get(i + k).map(|c| c.to_ascii_lowercase()).unwrap_or('\0');
    w(0) == 'h' && w(1) == 't' && w(2) == 't' && w(3) == 'p'
}

fn split_sentences(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if matches!(c, '。' | '！' | '？' | '；' | '\n' | '.' | '!' | '?') {
            if !cur.trim().is_empty() {
                parts.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

fn truncate_sentences(s: &str, cap: usize) -> String {
    let mut out = String::new();
    for sentence in split_sentences(s) {
        let candidate = if out.is_empty() {
            sentence.clone()
        } else {
            format!("{out}。{sentence}")
        };
        if char_count(&candidate) <= cap {
            out = candidate;
        } else {
            break;
        }
    }
    if out.is_empty() {
        out = truncate_chars(s, cap);
    }
    out
}

fn truncate_chars(s: &str, max: usize) -> String {
    if char_count(s) <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_util::{scripted, scripted_seq};

    /// 过闸门用的长正文（>100 字），短句版用于测抑制。
    fn long_content() -> String {
        "sqlite-vec 把向量检索能力塞进 SQLite，于是本地库不需要另起一个向量服务进程就能做相似度查询。\
         落地时要关心三件事：虚表按当前 embedding 维度建立、模型换版后必须全量重建、以及写入向量和\
         结果版本的一致性，否则检索会拿到过期坐标。Windows 上还要先验证 MSVC 工具链能编过扩展。"
            .into()
    }

    #[test]
    fn short_content_is_not_summarized_at_all() {
        // 一句诗 / 一条路径：请求都不该发出（用必崩的 client 证明"未调用"）。
        let c = scripted_seq(vec![]);
        let o = run(&c, "人闲桂花落，夜静春山空。");
        assert_eq!(o.summary, "");
        assert!(!o.degraded); // 主动不生成，不是兜底失败
    }

    #[test]
    fn content_just_under_threshold_is_suppressed() {
        let s = "句".repeat(MIN_SUMMARIZE_CHARS - 1);
        let c = scripted_seq(vec![]);
        assert_eq!(run(&c, &s).summary, "");
    }

    /// 把正文垫过 100 字闸门（垫字尾随在末句之后，不影响抽取式兜底取前两句）。
    fn padded(s: &str) -> String {
        let mut out = s.to_string();
        while char_count(&out) < MIN_SUMMARIZE_CHARS + 20 {
            out.push('垫');
        }
        out
    }

    #[test]
    fn long_content_model_summary_ok() {
        let c = scripted(r#"{"ok":true,"data":{"summary":"这是一句话主旨，讲向量检索落地 SQLite。"}}"#);
        let o = run(&c, &long_content());
        assert!(o.summary.starts_with("这是一句话主旨"));
        assert!(!o.degraded);
    }

    #[test]
    fn overlong_summary_truncated_not_degraded() {
        let long = "句。".repeat(60); // 120 字 > 短内容档 50
        let c = scripted(&format!(r#"{{"ok":true,"data":{{"summary":"{long}"}}}}"#));
        let o = run(&c, &long_content());
        assert!(char_count(&o.summary) <= 50);
        assert!(!o.degraded);
    }

    #[test]
    fn model_failure_uses_extractive_fallback() {
        let c = scripted_seq(vec!["nope".into(), r#"{"ok":false,"error_code":"X","message":"y"}"#.into()]);
        let content = padded("第一句话讲这个。第二句话讲那个。第三句不该出现。");
        let o = run(&c, &content);
        assert!(o.degraded);
        assert!(o.summary.contains("第一句话讲这个"));
        assert!(o.summary.contains("第二句话讲那个"));
        assert!(!o.summary.contains("第三句"));
    }

    #[test]
    fn extractive_strips_urls() {
        let c = scripted("bad");
        let content = padded("结论见 https://example.com/doc 这里。第二句。第三句不要。");
        let o = run(&c, &content);
        assert!(o.degraded);
        assert!(!o.summary.contains("https://"));
    }

    #[test]
    fn empty_summary_string_is_fallback() {
        let c = scripted(r#"{"ok":true,"data":{"summary":"   "}}"#);
        let o = run(&c, &long_content());
        assert!(o.degraded);
        assert!(!o.summary.trim().is_empty());
    }
}
