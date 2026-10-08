//! OpenAI 兼容协议实现（覆盖 qwen / deepseek 等 `openai_compat` 供应商，01 §llm）。
//! 只做「组装请求体 + 解析响应」，网络发送交给注入的 `Transport`——
//! 真实 reqwest 实现待依赖批准（见 docs/待确认清单.md P5），故此处以 stub 完成离线可测核心。

use std::fmt;

use crate::error::AppError;
use super::provider::{ChatMessage, ChatOpts, ChatOutcome, Provider, Transport};

/// OpenAI 兼容端点。`api_key` 只存内存（由上层从 keyring 取），
/// 手写 `Debug` 将其掩码，确保「不落明文日志」（AGENTS §3 数据边界）。
pub struct OpenAiProvider<'t> {
    pub base_url: String,
    pub api_key: String,
    transport: &'t dyn Transport,
}

impl<'t> fmt::Debug for OpenAiProvider<'t> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl<'t> OpenAiProvider<'t> {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>, transport: &'t dyn Transport) -> Self {
        Self { base_url: base_url.into(), api_key: api_key.into(), transport }
    }

    /// 组装 OpenAI `/chat/completions` 请求体。
    pub fn build_request(messages: &[ChatMessage], opts: &ChatOpts) -> serde_json::Value {
        serde_json::json!({
            "model": opts.model,
            "messages": messages,
            "max_tokens": opts.max_tokens,
            "temperature": opts.temperature,
            "stream": false,
        })
    }

    /// 解析 200 响应体为助手文本 + token 计量；结构不符视为坏输出（走 04 §0 兜底，不 panic）。
    /// `usage` 缺失只影响计量，**不影响本次调用成功**（兼容端点常不回填）。
    pub fn parse_response(body: &str) -> Result<ChatOutcome, AppError> {
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|_| AppError::LlmBadOutput("响应非合法 JSON".into()))?;
        let content = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| AppError::LlmBadOutput("响应缺 choices[0].message.content".into()))?;
        Ok(ChatOutcome { content: content.to_string(), usage: super::usage::parse_usage(&v) })
    }
}

impl<'t> Provider for OpenAiProvider<'t> {
    fn chat(&self, messages: &[ChatMessage], opts: &ChatOpts) -> Result<ChatOutcome, AppError> {
        let body = Self::build_request(messages, opts);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let raw = self.transport.post_chat(&url, &self.api_key, &body, opts.timeout)?;
        Self::parse_response(&raw)
    }

    fn model(&self) -> String {
        // Provider 无状态模型字段：由调用方 ChatOpts.model 决定，这里回显通用标识。
        "openai_compat".into()
    }
}

/// 真实 HTTP 传输（reqwest blocking）。仅在 blocking 线程（`spawn_blocking`）中调用，
/// 避免在 tauri 的 tokio 运行时线程内直接跑 blocking 客户端。`api_key` 仅作参数、不落日志。
pub struct HttpTransport {
    client: reqwest::blocking::Client,
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self { client }
    }
}

/// 把 reqwest 传输错误映射到 AppError，保留「可重试性」语义（超时/连接=Transient）。
fn map_send_err(e: &reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::LlmTimeout
    } else if e.is_connect() {
        AppError::FetchFailed("无法连接上游".into())
    } else {
        AppError::FetchFailed("传输错误".into())
    }
}

impl Transport for HttpTransport {
    fn post_chat(
        &self,
        base_url: &str,
        api_key: &str,
        body: &serde_json::Value,
        timeout: std::time::Duration,
    ) -> Result<String, AppError> {
        let payload =
            serde_json::to_vec(body).map_err(|e| AppError::LlmBadOutput(format!("请求序列化失败: {e}")))?;
        let resp = self
            .client
            .post(base_url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(timeout)
            .body(payload)
            .send()
            .map_err(|e| map_send_err(&e))?;

        let status = resp.status();
        let code = status.as_u16();
        if code == 429 {
            return Err(AppError::LlmRateLimited);
        }
        if status.is_server_error() {
            return Err(AppError::FetchFailed(format!("上游 5xx: {code}")));
        }
        if status.is_client_error() {
            // 401/403 鉴权失败→视为未正确配置（非可重试，不降级）；其余 4xx→输入非法。
            // 不回传上游原文，避免泄漏与前端裸错误。
            if code == 401 || code == 403 {
                return Err(AppError::ConfigMissing);
            }
            return Err(AppError::InputInvalid(format!("上游 4xx: {code}")));
        }
        resp.text().map_err(|e| AppError::FetchFailed(e.to_string()))
    }
}

/// 测试/离线用传输：按脚本依次返回预设响应，并记录被调次数。
#[cfg(test)]
pub(crate) struct StubTransport {
    responses: std::cell::RefCell<std::vec::IntoIter<Result<String, AppError>>>,
    pub(crate) calls: std::cell::Cell<u32>,
    pub(crate) last_body: std::cell::RefCell<Option<serde_json::Value>>,
}

#[cfg(test)]
impl StubTransport {
    pub(crate) fn ok_sequence(responses: Vec<&str>) -> Self {
        Self {
            responses: std::cell::RefCell::new(
                responses.into_iter().map(|r| Ok(r.to_string())).collect::<Vec<_>>().into_iter(),
            ),
            calls: std::cell::Cell::new(0),
            last_body: std::cell::RefCell::new(None),
        }
    }
    pub(crate) fn err_sequence(responses: Vec<AppError>) -> Self {
        Self {
            responses: std::cell::RefCell::new(responses.into_iter().map(Err).collect::<Vec<_>>().into_iter()),
            calls: std::cell::Cell::new(0),
            last_body: std::cell::RefCell::new(None),
        }
    }
}

#[cfg(test)]
impl Transport for StubTransport {
    fn post_chat(
        &self,
        _base_url: &str,
        _api_key: &str,
        body: &serde_json::Value,
        _timeout: std::time::Duration,
    ) -> Result<String, AppError> {
        self.calls.set(self.calls.get() + 1);
        *self.last_body.borrow_mut() = Some(body.clone());
        self.responses.borrow_mut().next().unwrap_or_else(|| Ok(String::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn opts() -> ChatOpts {
        ChatOpts { model: "qwen3-max".into(), timeout: Duration::from_secs(20), max_tokens: 2048, temperature: 0.0 }
    }

    #[test]
    fn build_request_matches_openai_shape() {
        let req = OpenAiProvider::build_request(
            &[ChatMessage::system("sys"), ChatMessage::user("hi")],
            &opts(),
        );
        assert_eq!(req["model"], "qwen3-max");
        assert_eq!(req["temperature"], 0.0);
        assert_eq!(req["messages"][0]["role"], "system");
        assert_eq!(req["messages"][1]["content"], "hi");
    }

    #[test]
    fn parse_response_extracts_content() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"你好"}}]}"#;
        assert_eq!(OpenAiProvider::parse_response(body).unwrap().content, "你好");
    }

    /// 批次17：文本与计量同时从一次响应里取；缺 usage 不算失败，也不谎报成 0。
    #[test]
    fn parse_response_reads_usage_when_endpoint_returns_it() {
        let body = r#"{"choices":[{"message":{"content":"hi"}}],
                        "usage":{"prompt_tokens":131,"completion_tokens":47,"total_tokens":178}}"#;
        let out = OpenAiProvider::parse_response(body).unwrap();
        assert_eq!(out.content, "hi");
        assert_eq!(
            out.usage,
            Some(crate::llm::usage::Usage { prompt_tokens: Some(131), completion_tokens: Some(47) })
        );
        assert_eq!(OpenAiProvider::parse_response(r#"{"choices":[{"message":{"content":"x"}}]}"#)
            .unwrap()
            .usage,
            None);
    }

    #[test]
    fn parse_response_bad_shapes_yield_fallback_error_not_panic() {
        assert!(matches!(OpenAiProvider::parse_response("not json"), Err(AppError::LlmBadOutput(_))));
        assert!(matches!(
            OpenAiProvider::parse_response(r#"{"choices":[]}"#),
            Err(AppError::LlmBadOutput(_))
        ));
        assert!(matches!(
            OpenAiProvider::parse_response(r#"{"data":1}"#),
            Err(AppError::LlmBadOutput(_))
        ));
    }

    #[test]
    fn api_key_never_appears_in_debug() {
        let stub = StubTransport::ok_sequence(vec![]);
        let p = OpenAiProvider::new("https://api.example.com/v1", "sk-SECRET-12345", &stub);
        let dbg = format!("{p:?}");
        assert!(!dbg.contains("sk-SECRET-12345"));
        assert!(dbg.contains("REDACTED"));
    }

    #[test]
    fn chat_sends_to_completions_endpoint_and_parses() {
        let stub = StubTransport::ok_sequence(vec![
            r#"{"choices":[{"message":{"content":"答复"}}]}"#,
        ]);
        let p = OpenAiProvider::new("https://api.example.com/v1/", "k", &stub);
        let out = p.chat(&[ChatMessage::user("q")], &opts()).unwrap();
        assert_eq!(out.content, "答复");
        assert_eq!(stub.calls.get(), 1);
    }

    /// P5 验收「真调一条 ping prompt 成功」：需真实密钥，故默认忽略。
    /// 跑法：把 DeepSeek key 填入 `.env` 的 `DEEPSEEK_API_KEY`，然后
    /// `cargo test --lib -- --ignored real_ping_deepseek`。离线 CI 不跑此测。
    #[test]
    #[ignore = "需要 DEEPSEEK_API_KEY 真实密钥与网络"]
    fn real_ping_deepseek() {
        use std::time::Duration;
        crate::config::secrets::load_dotenv();
        let key = crate::config::secrets::api_key_for(crate::config::deepseek::PROVIDER)
            .expect("请先在 .env 设置 DEEPSEEK_API_KEY");
        let t = HttpTransport::new();
        let p = OpenAiProvider::new(crate::config::deepseek::BASE_URL, &key, &t);
        let opts = ChatOpts {
            model: crate::config::deepseek::CHAT_MODEL.into(),
            timeout: Duration::from_secs(30),
            max_tokens: 16,
            temperature: 0.0,
        };
        let out = p
            .chat(&[ChatMessage::user("只回复两个字母：ok")], &opts)
            .expect("真调用失败：检查密钥/网络/端点");
        assert!(!out.content.trim().is_empty(), "返回内容不应为空");
        // 顺带核对真端点是否回填 usage——决定统计面板会不会大面积「未知」。
        println!("deepseek ping -> {} | usage {:?}", out.content, out.usage);
    }
}
