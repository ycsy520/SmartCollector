//! Provider / Transport 抽象（01 清单 §llm）。
//! 把「面向任务的会话接口」与「网络发送」分层：`Provider::chat` 收消息返回文本，
//! 底层 `Transport` 只负责把已组装好的 OpenAI 兼容请求体发出去、拿回原始 JSON 文本。
//! 真实 HTTP 实现（reqwest）待依赖批准，测试用 stub 驱动，故 P5 离线核心零新依赖。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 一条会话消息（OpenAI `messages` 元素）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
}

/// 单次会话参数。`temperature` 对 JSON 任务取 0（04 §0 要求稳定结构化输出）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatOpts {
    pub model: String,
    pub timeout: Duration,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// 网络传输抽象。实现方负责 HTTP、超时与状态码→AppError 映射
/// （429→`LlmRateLimited`、超时→`LlmTimeout`、其余传输错→`FetchFailed`）。
/// `api_key` 仅作参数传入，实现内不得落日志。
pub trait Transport {
    fn post_chat(
        &self,
        base_url: &str,
        api_key: &str,
        body: &serde_json::Value,
        timeout: Duration,
    ) -> Result<String, AppError>;
}

/// 一次会话的产出：模型原始文本 + 上游回传的 token 计量。
/// `usage` 为 `None` 表示端点没回 `usage` 字段（兼容网关的常态），
/// **不得当成 0 计量**——统计面会把这类调用单独计数（02 §7.5）。
#[derive(Debug, Clone)]
pub struct ChatOutcome {
    pub content: String,
    pub usage: Option<super::usage::Usage>,
}

/// 面向流水线的高层接口：喂消息序列，吐模型原始文本（未解析）与计量。
pub trait Provider {
    fn chat(&self, messages: &[ChatMessage], opts: &ChatOpts) -> Result<ChatOutcome, AppError>;
    #[allow(dead_code)] // 实际 model 由 ChatOpts 携带；此回显供 worker 记 model_used
    fn model(&self) -> String;
}
