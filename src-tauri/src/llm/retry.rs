//! 重试策略（01 §llm）：指数退避、可重试分类、主端点耗尽后降级到备用端点（`llm.fallback`）。
//! 纯计算 + 注入 sleep，无 tokio、无真实时钟，离线可单测。

use std::thread;
use std::time::Duration;

use crate::config::RetryConfig;
use crate::error::{AppError, ErrorLevel};
use super::provider::{ChatMessage, ChatOpts, ChatOutcome, Provider};

/// 是否值得重试：仅临时错误（超时/429/坏输出/抓取失败/向量不可用）。
/// 客户端错误（校验/未配置/冲突）与致命错误（DB/密钥）立即上抛，不重试也不降级。
pub fn is_retryable(err: &AppError) -> bool {
    err.level() == ErrorLevel::Transient
}

/// 第 `attempt`（0 起）次重试前的退避毫秒：`base * 2^attempt`，饱和乘、封顶 `cap`。
pub fn backoff_ms(base: u64, attempt: u32, cap: u64) -> u64 {
    let factor = 2u64.checked_pow(attempt).unwrap_or(u64::MAX);
    base.saturating_mul(factor).min(cap)
}

/// 对单个 Provider 反复调用：初次 + 最多 `max_retries` 次重试，重试间退避。
/// 命中非可重试错误或次数用尽即返回。
fn call_one(
    provider: &dyn Provider,
    cfg: &RetryConfig,
    messages: &[ChatMessage],
    opts: &ChatOpts,
    sleep: &mut dyn FnMut(Duration),
) -> Result<ChatOutcome, AppError> {
    let mut last = AppError::LlmTimeout;
    for attempt in 0..=cfg.max_retries {
        match provider.chat(messages, opts) {
            Ok(text) => return Ok(text),
            Err(e) => {
                last = e;
                if !is_retryable(&last) || attempt == cfg.max_retries {
                    break;
                }
                sleep(Duration::from_millis(backoff_ms(
                    cfg.backoff_base_ms,
                    attempt,
                    cfg.backoff_max_ms,
                )));
            }
        }
    }
    Err(last)
}

/// 先主后备：主端点因**可重试**错误耗尽 → 降级到备用端点再试一轮。
/// 主端点返回非可重试错误（如未配置/校验失败）则直接上抛，不降级。
/// `sleep` 注入以便测试断言退避时长序列；生产传线程 sleep。
pub fn call_with_retry(
    primary: &dyn Provider,
    fallback: Option<&dyn Provider>,
    cfg: &RetryConfig,
    messages: &[ChatMessage],
    opts: &ChatOpts,
    sleep: &mut dyn FnMut(Duration),
) -> Result<ChatOutcome, AppError> {
    match call_one(primary, cfg, messages, opts, sleep) {
        Ok(out) => Ok(out),
        Err(primary_err) if is_retryable(&primary_err) => match fallback {
            Some(fb) => call_one(fb, cfg, messages, opts, sleep)
                // 备用也失败：报告主端点错误（更接近用户配置的第一现场），备错误链在日志侧另议。
                .map_err(|_| primary_err),
            None => Err(primary_err),
        },
        Err(primary_err) => Err(primary_err),
    }
}

/// 生产用退避睡眠（阻塞当前线程；worker 接入 tokio 后替换）。
#[allow(dead_code)] // 调用点在 P8 后台 worker 接入
pub fn thread_sleep(d: Duration) {
    thread::sleep(d);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use std::cell::Cell;

    struct Scripted {
        // 每次 chat 返回此 closure 给出的 Result；用 refcell 序列驱动。
        seq: std::cell::RefCell<std::vec::IntoIter<Result<ChatOutcome, AppError>>>,
        calls: Cell<u32>,
    }
    impl Scripted {
        fn new(items: Vec<Result<ChatOutcome, AppError>>) -> Self {
            Self { seq: std::cell::RefCell::new(items.into_iter()), calls: Cell::new(0) }
        }
    }
    impl Provider for Scripted {
        fn chat(&self, _m: &[ChatMessage], _o: &ChatOpts) -> Result<ChatOutcome, AppError> {
            self.calls.set(self.calls.get() + 1);
            self.seq.borrow_mut().next().unwrap_or(Err(AppError::LlmTimeout))
        }
        fn model(&self) -> String {
            "scripted".into()
        }
    }

    fn cfg() -> RetryConfig {
        RetryConfig { max_retries: 3, backoff_base_ms: 1000, backoff_max_ms: 30000 }
    }
    fn opts() -> ChatOpts {
        ChatOpts {
            model: "m".into(),
            timeout: Duration::from_secs(1),
            max_tokens: 100,
            temperature: 0.0,
        }
    }
    fn ok(s: &str) -> Result<ChatOutcome, AppError> {
        Ok(ChatOutcome { content: s.to_string(), usage: None })
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_ms(1000, 0, 30000), 1000);
        assert_eq!(backoff_ms(1000, 1, 30000), 2000);
        assert_eq!(backoff_ms(1000, 2, 30000), 4000);
        assert_eq!(backoff_ms(1000, 10, 30000), 30000); // 封顶
    }

    #[test]
    fn retries_then_succeeds_with_backoff_sequence() {
        let p = Scripted::new(vec![
            Err(AppError::LlmRateLimited),
            Err(AppError::LlmTimeout),
            ok("成功"),
        ]);
        let mut slept = Vec::new();
        let out = call_with_retry(
            &p,
            None,
            &cfg(),
            &[ChatMessage::user("x")],
            &opts(),
            &mut |d| slept.push(d),
        )
        .unwrap();
        assert_eq!(out.content, "成功");
        assert_eq!(p.calls.get(), 3);
        assert_eq!(slept, vec![Duration::from_millis(1000), Duration::from_millis(2000)]);
    }

    #[test]
    fn client_error_aborts_immediately_without_fallback() {
        let p = Scripted::new(vec![Err(AppError::ConfigMissing)]);
        let fb = Scripted::new(vec![ok("不该被调用")]);
        let out = call_with_retry(
            &p,
            Some(&fb),
            &cfg(),
            &[ChatMessage::user("x")],
            &opts(),
            &mut |_| {},
        );
        assert!(matches!(out, Err(AppError::ConfigMissing)));
        assert_eq!(p.calls.get(), 1); // 未重试
        assert_eq!(fb.calls.get(), 0); // 未降级
    }

    #[test]
    fn falls_back_to_secondary_after_primary_exhausted() {
        // 主：全部超时（max_retries=3 → 调用 4 次）。备：首次即成功。
        let p = Scripted::new(vec![
            Err(AppError::LlmTimeout),
            Err(AppError::LlmTimeout),
            Err(AppError::LlmTimeout),
            Err(AppError::LlmTimeout),
        ]);
        let fb = Scripted::new(vec![ok("备用答复")]);
        let out = call_with_retry(
            &p,
            Some(&fb),
            &cfg(),
            &[ChatMessage::user("x")],
            &opts(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(out.content, "备用答复");
        assert_eq!(p.calls.get(), 4); // 1 + 3 retries
        assert_eq!(fb.calls.get(), 1);
    }

    #[test]
    fn both_exhausted_reports_primary_error() {
        let p = Scripted::new((0..4).map(|_| Err(AppError::LlmTimeout)).collect());
        let fb = Scripted::new((0..4).map(|_| Err(AppError::LlmTimeout)).collect());
        let out = call_with_retry(
            &p,
            Some(&fb),
            &cfg(),
            &[ChatMessage::user("x")],
            &opts(),
            &mut |_| {},
        );
        assert!(matches!(out, Err(AppError::LlmTimeout)));
        assert_eq!(fb.calls.get(), 4);
    }
}
