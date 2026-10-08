//! Embedding 客户端（01 §vector）：复用 llm/ 的「协议组装 ↔ 网络传输」分层思路，
//! 把 OpenAI 兼容 `/embeddings` 的请求构造与响应解析放离线可测层，真实 HTTP 待 reqwest。
//! 用 config `llm.embedding`（provider/base_url/model/dim）。

use std::fmt;
use std::time::{Duration, Instant};

use crate::config::EmbeddingEndpoint;
use crate::error::AppError;
use crate::llm::usage::{parse_usage, CallReceipt, NoUsageRecorder, UsageRecorder};

/// 把一段文本编码成定长向量。
pub trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, AppError>;
    #[allow(dead_code)] // model/dim 供 vector_refs 落库与重建校验，P8 worker 消费
    fn model(&self) -> String;
    #[allow(dead_code)]
    fn dim(&self) -> usize;
}

/// 网络传输抽象（与 llm::provider::Transport 同构，端点不同）。
/// 实现方负责 HTTP/超时/状态码→AppError；`api_key` 仅参数传入、不得落日志。
pub trait EmbeddingTransport {
    fn post_embeddings(
        &self,
        base_url: &str,
        api_key: &str,
        body: &serde_json::Value,
        timeout: Duration,
    ) -> Result<String, AppError>;
}

/// OpenAI 兼容 embedding 端点。`api_key` 只存内存，手写 Debug 掩码。
pub struct OpenAiEmbedder<'t> {
    endpoint: EmbeddingEndpoint,
    api_key: String,
    transport: &'t dyn EmbeddingTransport,
    usage: &'t dyn UsageRecorder,
}

impl<'t> fmt::Debug for OpenAiEmbedder<'t> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiEmbedder")
            .field("model", &self.endpoint.model)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl<'t> OpenAiEmbedder<'t> {
    pub fn new(
        endpoint: EmbeddingEndpoint,
        api_key: impl Into<String>,
        transport: &'t dyn EmbeddingTransport,
    ) -> Self {
        // 默认不计量：计量去向由调用方显式接上（未接时统计面板诚实显示"无记录"）。
        Self { endpoint, api_key: api_key.into(), transport, usage: &NoUsageRecorder }
    }

    /// 接上计量去向（`db::usage::DbUsageRecorder`）。批次17 只计**处理与再加工**路的编码；
    /// 检索期的查询编码在独立线程里现建端点、不计入（面板文案据此说话，见 02 §7.5 范围注记）。
    pub fn with_usage(mut self, usage: &'t dyn UsageRecorder) -> Self {
        self.usage = usage;
        self
    }

    pub fn build_request(&self, text: &str) -> serde_json::Value {
        serde_json::json!({
            "model": self.endpoint.model,
            "input": text,
            "dimensions": self.endpoint.dim,
        })
    }

    /// 解析 `{"data":[{"embedding":[...]}]}`；结构不符→坏输出（走兜底，不 panic）。
    pub fn parse_response(body: &str) -> Result<Vec<f32>, AppError> {
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|_| AppError::LlmBadOutput("embedding 响应非 JSON".into()))?;
        let arr = v
            .get("data")
            .and_then(|d| d.get(0))
            .and_then(|d| d.get("embedding"))
            .and_then(|e| e.as_array())
            .ok_or_else(|| AppError::LlmBadOutput("embedding 响应缺 data[0].embedding".into()))?;
        let out: Option<Vec<f32>> = arr.iter().map(|x| x.as_f64().map(|n| n as f32)).collect();
        out.ok_or_else(|| AppError::LlmBadOutput("embedding 含非数值分量".into()))
    }
}

impl<'t> Embedder for OpenAiEmbedder<'t> {
    fn embed(&self, text: &str) -> Result<Vec<f32>, AppError> {
        let url = format!("{}/embeddings", self.endpoint.base_url.trim_end_matches('/'));
        let body = self.build_request(text);
        let started = Instant::now();
        let raw = match self
            .transport
            .post_embeddings(&url, &self.api_key, &body, Duration::from_secs(60))
        {
            Ok(raw) => raw,
            Err(e) => {
                self.record(None, false, Some(e.code().to_string()), started);
                return Err(e);
            }
        };
        let vec = match Self::parse_response(&raw) {
            Ok(v) => v,
            Err(e) => {
                self.record(Some(&raw), false, Some(e.code().to_string()), started);
                return Err(e);
            }
        };
        // 维度与配置不符：视为上游异常，交由调用方（全量重建/降级）处理。
        // 一次调用**只落一条**回执：成功账要压在维度校验之后，否则同一次请求被记成
        // 一成一败两笔，面板的总调用数与失败率都会虚高。
        if vec.len() as u32 != self.endpoint.dim {
            let e = AppError::VectorUnavailable(format!(
                "维度不符：配置 {}，返回 {}",
                self.endpoint.dim,
                vec.len()
            ));
            self.record(Some(&raw), false, Some(e.code().to_string()), started);
            return Err(e);
        }
        self.record(Some(&raw), true, None, started);
        Ok(vec)
    }
    fn model(&self) -> String {
        self.endpoint.model.clone()
    }
    fn dim(&self) -> usize {
        self.endpoint.dim as usize
    }
}

impl<'t> OpenAiEmbedder<'t> {
    /// 落一条编码计量。embedding 只有输入方向，`completion_tokens` 恒为 `None`；
    /// 响应体读不出 `usage` 时记"未知"而不是 0（与 chat 路同一口径）。
    fn record(
        &self,
        raw: Option<&str>,
        ok: bool,
        error_code: Option<String>,
        started: Instant,
    ) {
        let usage = raw
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| parse_usage(&v));
        self.usage.record(&CallReceipt {
            endpoint: "embedding",
            task: "embed".into(),
            model: self.endpoint.model.clone(),
            usage,
            ok,
            error_code,
            latency_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        });
    }
}

/// 真实 HTTP 传输（reqwest blocking），与 `llm::openai::HttpTransport` 同构、错误映射一致，
/// 保留「可重试性」语义：429→限流、5xx/连接→可重试、401/403→未配置、其余 4xx→输入非法。
/// 不回传上游裸响应。仅在独立 std 线程调用（blocking 客户端会建自己的 tokio 运行时）。
pub struct HttpEmbeddingTransport {
    client: reqwest::blocking::Client,
}

impl Default for HttpEmbeddingTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpEmbeddingTransport {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self { client }
    }
}

impl EmbeddingTransport for HttpEmbeddingTransport {
    fn post_embeddings(
        &self,
        url: &str,
        api_key: &str,
        body: &serde_json::Value,
        timeout: Duration,
    ) -> Result<String, AppError> {
        let payload =
            serde_json::to_vec(body).map_err(|e| AppError::LlmBadOutput(format!("请求序列化失败: {e}")))?;
        let resp = self
            .client
            .post(url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(timeout)
            .body(payload)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    AppError::LlmTimeout
                } else if e.is_connect() {
                    AppError::FetchFailed("无法连接上游".into())
                } else {
                    AppError::FetchFailed("传输错误".into())
                }
            })?;
        let status = resp.status();
        let code = status.as_u16();
        if code == 429 {
            return Err(AppError::LlmRateLimited);
        }
        if status.is_server_error() {
            return Err(AppError::FetchFailed(format!("上游 5xx: {code}")));
        }
        if status.is_client_error() {
            if code == 401 || code == 403 {
                return Err(AppError::ConfigMissing);
            }
            return Err(AppError::InputInvalid(format!("上游 4xx: {code}")));
        }
        resp.text().map_err(|e| AppError::FetchFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct StubT {
        resp: String,
        calls: Cell<u32>,
    }
    impl EmbeddingTransport for StubT {
        fn post_embeddings(
            &self,
            _u: &str,
            _k: &str,
            _b: &serde_json::Value,
            _t: Duration,
        ) -> Result<String, AppError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.resp.clone())
        }
    }

    fn ep(dim: u32) -> EmbeddingEndpoint {
        EmbeddingEndpoint {
            provider: "openai_compat".into(),
            base_url: "https://api.example.com/v1".into(),
            model: "text-embedding-v3".into(),
            dim,
        }
    }

    #[test]
    fn build_request_matches_openai_embeddings_shape() {
        let stub = StubT { resp: "{}".into(), calls: Cell::new(0) };
        let e = OpenAiEmbedder::new(ep(3), "k", &stub);
        let req = e.build_request("hello");
        assert_eq!(req["model"], "text-embedding-v3");
        assert_eq!(req["input"], "hello");
        assert_eq!(req["dimensions"], 3);
    }

    #[test]
    fn parse_and_embed_roundtrip_with_dim_check() {
        let stub = StubT {
            resp: r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#.into(),
            calls: Cell::new(0),
        };
        let e = OpenAiEmbedder::new(ep(3), "k", &stub);
        let vec = e.embed("hello").unwrap();
        assert_eq!(vec.len(), 3);
        assert_eq!(stub.calls.get(), 1);
    }

    #[test]
    fn dim_mismatch_surfaces_as_error() {
        let stub = StubT {
            resp: r#"{"data":[{"embedding":[0.1,0.2]}]}"#.into(), // 配 3 维却返回 2
            calls: Cell::new(0),
        };
        let e = OpenAiEmbedder::new(ep(3), "k", &stub);
        assert!(matches!(e.embed("x"), Err(AppError::VectorUnavailable(_))));
    }

    /// 批次17：编码也要落计量；**失败同样要留痕**（否则面板把"上游挂了"读成"今天很省"）。
    #[test]
    fn embed_records_usage_and_failure() {
        struct Collect {
            hits: std::sync::Mutex<Vec<CallReceipt>>,
        }
        impl UsageRecorder for Collect {
            fn record(&self, r: &CallReceipt) {
                self.hits.lock().unwrap().push(r.clone());
            }
        }
        let rec = Collect { hits: std::sync::Mutex::new(Vec::new()) };

        let ok_stub = StubT {
            resp: r#"{"data":[{"embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":9}}"#.into(),
            calls: Cell::new(0),
        };
        OpenAiEmbedder::new(ep(3), "k", &ok_stub)
            .with_usage(&rec)
            .embed("hello")
            .unwrap();

        let dim_stub = StubT { resp: r#"{"data":[{"embedding":[0.1]}]}"#.into(), calls: Cell::new(0) };
        assert!(OpenAiEmbedder::new(ep(3), "k", &dim_stub).with_usage(&rec).embed("x").is_err());

        let hits = rec.hits.lock().unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].endpoint, hits[0].task.as_str(), hits[0].ok), ("embedding", "embed", true));
        assert_eq!(hits[0].usage.and_then(|u| u.prompt_tokens), Some(9));
        assert_eq!(
            hits[0].usage.and_then(|u| u.completion_tokens),
            None,
            "embedding 没有输出方向，不得编造"
        );
        assert!(!hits[1].ok);
        assert_eq!(hits[1].error_code.as_deref(), Some("E_VECTOR_UNAVAILABLE"));
    }

    #[test]
    fn bad_response_no_panic() {
        assert!(matches!(
            OpenAiEmbedder::parse_response("nope"),
            Err(AppError::LlmBadOutput(_))
        ));
        assert!(matches!(
            OpenAiEmbedder::parse_response(r#"{"data":[]}"#),
            Err(AppError::LlmBadOutput(_))
        ));
        assert!(matches!(
            OpenAiEmbedder::parse_response(r#"{"data":[{"embedding":["a","b"]}]}"#),
            Err(AppError::LlmBadOutput(_))
        ));
    }

    #[test]
    fn api_key_masked_in_debug() {
        let stub = StubT { resp: "{}".into(), calls: Cell::new(0) };
        let e = OpenAiEmbedder::new(ep(3), "sk-SECRET", &stub);
        assert!(!format!("{e:?}").contains("sk-SECRET"));
    }
}
