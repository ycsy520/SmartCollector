//! LlmRouter：按任务类型/成本选择模型档位（04 §8），并驱动重试/降级（retry）。
//! 真实网络由注入的 `Transport` 承担；reqwest 实现待依赖批准，此处以 stub 完成离线核心。

pub mod openai;
pub mod provider;
pub mod retry;
pub mod usage;

use std::time::{Duration, Instant};

use crate::config::{LlmEndpoint, RetryConfig};
use crate::error::AppError;
use openai::OpenAiProvider;
use provider::{ChatMessage, ChatOpts, ChatOutcome, Transport};
use retry::call_with_retry;
use usage::{CallReceipt, NoUsageRecorder, UsageRecorder};

/// 任务成本档位（04 §8「建议模型档位」+ 超时列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTier {
    Small,
    Medium,
    Large,
}

impl TaskTier {
    /// 档位建议超时秒（受端点配置上限进一步收敛）。
    fn suggested_timeout_secs(self) -> u64 {
        match self {
            TaskTier::Small => 20,
            TaskTier::Medium => 45,
            TaskTier::Large => 60,
        }
    }
}

/// 任务名 → 档位（04 §8）。未知任务归为 Medium，保守给足时间。
pub fn tier_for(task: &str) -> TaskTier {
    match task {
        "classify" | "tag" | "extract_links" => TaskTier::Small,
        "summarize" | "skill_run" | "habit_infer" => TaskTier::Medium,
        "qa_retrieve" => TaskTier::Large,
        _ => TaskTier::Medium,
    }
}

/// 主/备端点 + 重试配置 + 内存密钥 + 注入传输 + 计量去向。
pub struct LlmRouter<'t> {
    transport: &'t dyn Transport,
    primary: LlmEndpoint,
    fallback: Option<LlmEndpoint>,
    api_key: String,
    fallback_api_key: Option<String>,
    retry: RetryConfig,
    usage: &'t dyn UsageRecorder,
}

impl<'t> std::fmt::Debug for LlmRouter<'t> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmRouter")
            .field("primary", &self.primary.model)
            .field("fallback", &self.fallback.as_ref().map(|e| e.model.clone()))
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl<'t> LlmRouter<'t> {
    pub fn new(
        transport: &'t dyn Transport,
        primary: LlmEndpoint,
        fallback: Option<LlmEndpoint>,
        api_key: impl Into<String>,
        fallback_api_key: Option<String>,
        retry: RetryConfig,
    ) -> Self {
        // 默认不落计量：连接池在调用方手里，没显式接上之前不该假装统计过（面板会诚实显示"无记录"）。
        Self { transport, primary, fallback, api_key: api_key.into(), fallback_api_key, retry, usage: &NoUsageRecorder }
    }

    /// 接上计量去向（`db::usage::DbUsageRecorder`）。worker / 命令层拿到连接后调用一次。
    pub fn with_usage(mut self, usage: &'t dyn UsageRecorder) -> Self {
        self.usage = usage;
        self
    }

    /// 为某任务组装会话参数：模型取主端点，超时=档位建议值与端点上限的较小者。
    pub fn opts_for(&self, task: &str) -> ChatOpts {
        let tier = tier_for(task);
        let cap = self.primary.timeout_s as u64;
        ChatOpts {
            model: self.primary.model.clone(),
            timeout: Duration::from_secs(tier.suggested_timeout_secs().min(cap)),
            max_tokens: self.primary.max_tokens,
            temperature: 0.0,
        }
    }

    /// 主→备发起一次会话（含退避重试/降级），并把一次回执交给计量去向。
    /// `sleep` 注入以便测试；生产传 `retry::thread_sleep`。
    /// `tier_task` 决定超时档位（04 §8），`label_task` 决定计量归属（统计面板的「任务」维度）。
    /// 两者在 worker 里**故意不同**：四任务共用一个 client，档位按长文留余量、归属按真实任务名。
    pub fn complete(
        &self,
        tier_task: &str,
        label_task: &str,
        messages: &[ChatMessage],
        sleep: &mut dyn FnMut(Duration),
    ) -> Result<ChatOutcome, AppError> {
        let opts = self.opts_for(tier_task);
        // 降级到备用端点时上游实际跑的是备模型，这里记的是**请求里的模型名**（与
        // processing_results.model_used 同一口径）。要分清得主备各带 model，属过度设计。
        let model = opts.model.clone();
        let started = Instant::now();
        let primary =
            OpenAiProvider::new(self.primary.base_url.as_str(), self.api_key.as_str(), self.transport);
        let fb = self.fallback.as_ref().map(|e| {
            OpenAiProvider::new(
                e.base_url.as_str(),
                self.fallback_api_key.as_deref().unwrap_or(""),
                self.transport,
            )
        });
        let result = call_with_retry(
            &primary,
            fb.as_ref().map(|p| p as &dyn provider::Provider),
            &self.retry,
            messages,
            &opts,
            sleep,
        );
        let latency_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let receipt = |ok: bool, usage: Option<usage::Usage>, error_code: Option<String>| CallReceipt {
            endpoint: "chat",
            task: label_task.to_string(),
            model: model.clone(),
            usage,
            ok,
            error_code,
            latency_ms,
        };
        match &result {
            Ok(out) => self.usage.record(&receipt(true, out.usage, None)),
            Err(e) => self.usage.record(&receipt(false, None, Some(e.code().to_string()))),
        }
        result
    }
}

/// 把 Router 适配成 P4 流水线的 `LlmClient`（prompt → 单条 user 消息）。
/// worker/命令在 P8 接入后消费；先行开放以保持 P4→P5 打通。
#[allow(dead_code)]
pub struct RouterClient<'t> {
    pub router: &'t LlmRouter<'t>,
    pub task: &'static str,
}

#[allow(dead_code)]
impl<'t> crate::agent::LlmClient for RouterClient<'t> {
    /// `task` 由发起任务自带（`classify` / `tag` / …），决定**计量归属**；
    /// 超时**档位**仍按 client 挂的 `self.task`——worker 统一给长文留余量，
    /// 不能让某个任务的 20s 短档把整条流水线卡死。
    fn complete(&self, task: &str, prompt: &str) -> Result<String, AppError> {
        self.router
            .complete(self.task, task, &[ChatMessage::user(prompt)], &mut retry::thread_sleep)
            .map(|o| o.content)
    }
    fn model(&self) -> String {
        self.router.primary.model.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmEndpoint;
    use crate::error::AppError;
    use openai::StubTransport;

    fn endpoint(model: &str, timeout_s: u32) -> LlmEndpoint {
        LlmEndpoint {
            provider: "openai_compat".into(),
            base_url: "https://api.example.com/v1".into(),
            model: model.into(),
            timeout_s,
            max_tokens: 2048,
        }
    }
    fn retry() -> RetryConfig {
        RetryConfig { max_retries: 2, backoff_base_ms: 100, backoff_max_ms: 1000 }
    }
    fn msg() -> Vec<ChatMessage> {
        vec![ChatMessage::user("x")]
    }

    #[test]
    fn tier_mapping_matches_04() {
        assert_eq!(tier_for("classify"), TaskTier::Small);
        assert_eq!(tier_for("summarize"), TaskTier::Medium);
        assert_eq!(tier_for("qa_retrieve"), TaskTier::Large);
        assert_eq!(tier_for("unknown"), TaskTier::Medium);
    }

    #[test]
    fn opts_clamp_task_timeout_to_endpoint_cap() {
        let t = StubTransport::ok_sequence(vec![]);
        // 端点超时 10s < classify 建议 20s → 取 10s
        let r = LlmRouter::new(&t, endpoint("m", 10), None, "k", None, retry());
        assert_eq!(r.opts_for("classify").timeout, Duration::from_secs(10));
        // 端点 60s > summarize 建议 45s → 取 45s
        let r2 = LlmRouter::new(&t, endpoint("m", 60), None, "k", None, retry());
        assert_eq!(r2.opts_for("summarize").timeout, Duration::from_secs(45));
    }

    #[test]
    fn router_primary_success() {
        let t = StubTransport::ok_sequence(vec![
            r#"{"choices":[{"message":{"content":"ok"}}]}"#,
        ]);
        let r = LlmRouter::new(&t, endpoint("qwen3-max", 60), None, "k", None, retry());
        let out = r.complete("classify", "classify", &msg(), &mut |_| {}).unwrap();
        assert_eq!(out.content, "ok");
    }

    #[test]
    fn router_reports_error_when_primary_exhausted() {
        // 单端点：max_retries=2 → 3 次调用后上抛超时。降级到备用端点的分支在 retry.rs 覆盖。
        let primary = StubTransport::err_sequence(vec![
            AppError::LlmTimeout,
            AppError::LlmTimeout,
            AppError::LlmTimeout,
        ]);
        let r = LlmRouter::new(&primary, endpoint("m", 60), None, "k", None, retry());
        let out = r.complete("classify", "classify", &msg(), &mut |_| {});
        assert!(matches!(out, Err(AppError::LlmTimeout)));
        assert_eq!(primary.calls.get(), 3);
    }

    /// 批次17：一次会话无论成败都该留下一条回执，且**计量不能吞掉调用本身的返回值**。
    #[test]
    fn router_records_success_and_failure_receipts() {
        // 记录器要求 Send + Sync（与生产同一条约束），故用 Mutex 而非 RefCell。
        struct Collect {
            hits: std::sync::Mutex<Vec<CallReceipt>>,
        }
        impl UsageRecorder for Collect {
            fn record(&self, r: &CallReceipt) {
                self.hits.lock().unwrap().push(r.clone());
            }
        }
        let rec = Collect { hits: std::sync::Mutex::new(Vec::new()) };
        let ok = StubTransport::ok_sequence(vec![
            r#"{"choices":[{"message":{"content":"y"}}],"usage":{"prompt_tokens":11,"completion_tokens":2}}"#,
        ]);
        LlmRouter::new(&ok, endpoint("qwen3-max", 60), None, "k", None, retry())
            .with_usage(&rec)
            .complete("summarize", "tag", &msg(), &mut |_| {})
            .unwrap();

        let bad = StubTransport::err_sequence(vec![
            AppError::LlmTimeout,
            AppError::LlmTimeout,
            AppError::LlmTimeout,
        ]);
        let _ = LlmRouter::new(&bad, endpoint("deepseek-chat", 60), None, "k", None, retry())
            .with_usage(&rec)
            .complete("classify", "classify", &msg(), &mut |_| {});

        let hits = rec.hits.lock().unwrap();
        assert_eq!(hits.len(), 2, "成功与失败各一条");
        let s = &hits[0];
        assert_eq!((s.endpoint, s.task.as_str(), s.model.as_str(), s.ok), ("chat", "tag", "qwen3-max", true));
        assert_eq!(s.usage.and_then(|u| u.prompt_tokens), Some(11));
        assert_eq!(s.usage.and_then(|u| u.completion_tokens), Some(2));
        assert!(s.error_code.is_none());
        let f = &hits[1];
        assert!(!f.ok);
        assert_eq!(f.error_code.as_deref(), Some("E_LLM_TIMEOUT"));
        assert_eq!(f.usage, None, "失败的调用不该编造 token 数");
        assert!(f.latency_ms >= 0);
    }

    #[test]
    fn router_debug_masks_keys() {
        let t = StubTransport::ok_sequence(vec![]);
        let r = LlmRouter::new(&t, endpoint("m", 60), None, "sk-TOPSECRET", None, retry());
        let d = format!("{r:?}");
        assert!(!d.contains("sk-TOPSECRET"));
    }

    #[test]
    fn adapter_implements_agent_llm_client() {
        let t = StubTransport::ok_sequence(vec![
            r#"{"choices":[{"message":{"content":"hello"}}]}"#,
        ]);
        let r = LlmRouter::new(&t, endpoint("qwen3-max", 60), None, "k", None, retry());
        let client = RouterClient { router: &r, task: "classify" };
        assert_eq!(crate::agent::LlmClient::model(&client), "qwen3-max");
        let raw = crate::agent::LlmClient::complete(&client, "classify", "hi").unwrap();
        assert_eq!(raw, "hello");
    }
}
