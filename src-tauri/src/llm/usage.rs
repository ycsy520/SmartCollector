//! 大模型调用计量（批次17「使用统计」的数据面）。
//! 只记**元数据**：时刻、端点、任务、模型、token 数、成败、错误码、耗时。
//! 正文 / prompt / 响应原文 / API Key 一律不进这里（AGENTS §3 数据边界，
//! 与「隐私内容不经 AI、检测不走网络」是同一条红线；统计面板也不该变成第二份原文副本）。

use serde::{Deserialize, Serialize};

/// 上游返回的 token 计量。字段 `None` = 该端点没回 `usage`，
/// **不得当成 0 参与均值/合计**——前端要把"未知"和"零"分开显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
}

impl Usage {
    /// 两个方向都没数据时视为「本次调用没有可计量的 token」。
    pub fn or_none(self) -> Option<Usage> {
        if self.prompt_tokens.is_some() || self.completion_tokens.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

/// 一次会话/编码的回执。**一次逻辑调用记一行**（其内部退避重试合并在同一次里，
/// 因为重试次数需要 `retry` 层上抛才能拿到，为它改三个签名不值得——耗时/token 只统计最后一次）。
#[derive(Debug, Clone)]
pub struct CallReceipt {
    /// `"chat"` | `"embedding"`
    pub endpoint: &'static str,
    /// 任务名（`classify` / `tag` / `extract_links` / `summarize` / `skill_run` / `test_connection` / `embed`）
    pub task: String,
    pub model: String,
    pub usage: Option<Usage>,
    pub ok: bool,
    pub error_code: Option<String>,
    pub latency_ms: i64,
}

/// 回执的去向。由持久层实现（`db::usage::DbUsageRecorder`），llm 层只认这个接口，
/// 保持"网络层不依赖 db"的分层边界（01 清单 §分层）。
/// `Send + Sync` 是硬要求：`run_skill` 会把整个 router 移进独立 std 线程（blocking 客户端
/// 不能在 tauri 的 tokio 运行时线程里跑），实现必须能跨线程——所以它持连接池，不借连接。
pub trait UsageRecorder: Send + Sync {
    fn record(&self, receipt: &CallReceipt);
}

/// 未接库 / 单测用：什么都不做。
pub struct NoUsageRecorder;

impl UsageRecorder for NoUsageRecorder {
    fn record(&self, _receipt: &CallReceipt) {}
}

/// 从 OpenAI 兼容响应体取 `usage`（chat 与 embedding 同一字段名）。
/// 缺失、非对象、类型不符一律返回 `None`——这是**兼容行为**，不是错误：
/// 不少兼容端点（尤其自建网关）不回填 usage，走兜底而不是让整次调用失败。
pub fn parse_usage(v: &serde_json::Value) -> Option<Usage> {
    let u = v.get("usage")?;
    let pick = |key: &str| u.get(key).and_then(|n| n.as_i64());
    Usage {
        prompt_tokens: pick("prompt_tokens"),
        completion_tokens: pick("completion_tokens"),
    }
    .or_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_usage_reads_both_directions() {
        let v = serde_json::json!({"usage": {"prompt_tokens": 120, "completion_tokens": 30}});
        assert_eq!(
            parse_usage(&v),
            Some(Usage { prompt_tokens: Some(120), completion_tokens: Some(30) })
        );
    }

    #[test]
    fn embedding_usage_has_only_prompt() {
        let v = serde_json::json!({"usage": {"prompt_tokens": 7}});
        assert_eq!(
            parse_usage(&v),
            Some(Usage { prompt_tokens: Some(7), completion_tokens: None })
        );
    }

    #[test]
    fn missing_or_malformed_usage_is_none_not_error() {
        assert_eq!(parse_usage(&serde_json::json!({})), None);
        assert_eq!(parse_usage(&serde_json::json!({"usage": null})), None);
        assert_eq!(parse_usage(&serde_json::json!({"usage": "oops"})), None);
        // 有 usage 但两个字段都不是数字 → 仍是「未知」，不能报成 0
        assert_eq!(parse_usage(&serde_json::json!({"usage": {"prompt_tokens": "x"}})), None);
    }
}
