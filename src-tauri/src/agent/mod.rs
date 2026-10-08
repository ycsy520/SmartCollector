//! AgentPipeline：编排 classify / tag / summarize / extract_links 四任务（04 §6/§8）。
//! 各任务独立兜底，全部返回后聚合 degraded。P4 用同步 + `LlmClient` trait 抽象，
//! mock 可测；异步/tokio 并行与真实网络留待 P5（届时评估引入依赖）。

pub mod output;
pub mod prompt;
pub mod task_classify;
pub mod task_extract_links;
pub mod task_summarize;
pub mod task_tag;
pub mod worker;

use serde_json::Value;

use crate::db::results::ExtractedLink;
use crate::error::AppError;
use output::Envelope;

/// LLM 传输抽象。P4 由 mock 实现驱动单测；P5 由 llm::OpenAiProvider 实现。
pub trait LlmClient {
    /// 返回模型的原始文本输出（尚未解析）；网络/超时错误以 AppError 上抛，由 call_json 重试。
    /// `task` 是发起方的任务名（`classify` / `tag` / `extract_links` / `summarize` / `skill_run`），
    /// 只用于**计量归属**（批次17 统计面板的「任务」维度），不参与提示词。
    fn complete(&self, task: &str, prompt: &str) -> Result<String, AppError>;
    /// 实际使用的模型标识，落 processing_results.model_used。
    fn model(&self) -> String;
}

/// 调一次并解析信封；不可用时按 04 §0 追加"仅输出 JSON"重试一次。
/// 返回 data 对象（ok=true）；两次都不行 → None（交调用方兜底）。
pub(crate) fn call_json(client: &dyn LlmClient, task: &str, prompt: &str) -> Option<Value> {
    for attempt in 0..2 {
        let text = if attempt == 0 {
            prompt.to_string()
        } else {
            format!("{prompt}{}", prompt::RETRY_SUFFIX)
        };
        if let Ok(raw) = client.complete(task, &text) {
            if let Some(env) = Envelope::parse(&raw) {
                if let Some(data) = env.data() {
                    return Some(data.clone());
                }
            }
        }
    }
    None
}

/// 与 `call_json` 同规则解析信封，但**上抛网络/解析错误**（不吞成兜底）。
/// 用于无兜底语义的单发任务（如 `run_skill`）：调用方据 `AppError::code()` 回传 E_LLM_*。
/// 网络类错误立即上抛不重复请求；仅"拿到文本但解析失败"才追加一次重试后缀。
pub(crate) fn call_json_checked(client: &dyn LlmClient, task: &str, prompt: &str) -> Result<Value, AppError> {
    let mut last = AppError::LlmBadOutput("输出无法解析为 JSON".into());
    for attempt in 0..2 {
        let text = if attempt == 0 {
            prompt.to_string()
        } else {
            format!("{prompt}{}", prompt::RETRY_SUFFIX)
        };
        match client.complete(task, &text) {
            Ok(raw) => {
                if let Some(env) = Envelope::parse(&raw) {
                    if let Some(data) = env.data() {
                        return Ok(data.clone());
                    }
                    if let Some(code) = env.error_code {
                        // 模型自报失败（ok=false）：交调用方按 E_LLM_BAD_OUTPUT 处理，保留信息。
                        last = AppError::LlmBadOutput(format!("模型返回失败: {code}"));
                    }
                }
            }
            Err(e) => return Err(e), // 网络/超时/限流：透传原始错误码，不再重试请求
        }
    }
    Err(last)
}

/// pipeline 聚合产出，字段与 db::results::insert_result 入参一一对应。
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // 生产调用点（commands/worker）在 P7 接入，先行开放 API
pub struct PipelineOutput {
    pub category: String,
    pub subcategory: Option<String>,
    pub tags: Vec<String>,
    pub summary: String,
    pub links: Vec<ExtractedLink>,
    pub degraded: bool,
    pub model_used: String,
}

pub struct AgentPipeline;

#[allow(dead_code)]
impl AgentPipeline {
    /// 同步串行跑四任务。四任务无相互依赖，P5 换 tokio join 并行；兜底语义不变。
    pub fn run(client: &dyn LlmClient, content: &str) -> PipelineOutput {
        let cls = task_classify::run(client, content);
        let tag = task_tag::run(client, content);
        let sum = task_summarize::run(client, content);
        let links = task_extract_links::run(client, content);
        PipelineOutput {
            category: cls.category,
            subcategory: cls.subcategory,
            tags: tag.tags,
            summary: sum.summary,
            links,
            degraded: cls.degraded || tag.degraded || sum.degraded,
            model_used: client.model(),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 每次调用都返回同一响应。
    pub struct Scripted {
        responses: Vec<String>,
        idx: Rc<RefCell<usize>>,
        model: String,
    }

    pub fn scripted(resp: &str) -> Scripted {
        Scripted {
            responses: vec![resp.to_string()],
            idx: Rc::new(RefCell::new(0)),
            model: "mock".into(),
        }
    }

    /// 依次返回给定响应；用尽后重复最后一个（用于验证重试）。
    pub fn scripted_seq(responses: Vec<String>) -> Scripted {
        Scripted {
            responses,
            idx: Rc::new(RefCell::new(0)),
            model: "mock".into(),
        }
    }

    impl LlmClient for Scripted {
        fn complete(&self, _task: &str, _prompt: &str) -> Result<String, AppError> {
            let mut i = self.idx.borrow_mut();
            let resp = self
                .responses
                .get(*i)
                .or_else(|| self.responses.last())
                .cloned()
                .unwrap_or_default();
            *i += 1;
            Ok(resp)
        }
        fn model(&self) -> String {
            self.model.clone()
        }
    }

    #[test]
    fn pipeline_aggregates_and_marks_degraded() {
        // classify 正常，tag 兜底（返回分类词被过滤后 <2）→ degraded
        let client = scripted_seq(vec![
            r#"{"ok":true,"data":{"category":"技术","subcategory":"数据库"}}"#.into(), // classify
            r#"{"ok":true,"data":{"tags":["技术","生活"]}}"#.into(), // tag → 全过滤 → 兜底
            r#"{"ok":true,"data":{"summary":"sqlite-vec 摘要"}}"#.into(), // summarize
            r#"{"ok":true,"data":{"links":[],"extra":[]}}"#.into(), // extract
        ]);
        let out = AgentPipeline::run(&client, "sqlite-vec 让向量检索跑在 SQLite 里。");
        assert_eq!(out.category, "技术");
        assert!(out.tags.is_empty());
        assert!(out.degraded); // 因 tag 兜底
        assert!(out.summary.is_empty(), "短内容不产出摘要（04 §3.1）");
        assert_eq!(out.model_used, "mock");
    }

    #[test]
    fn total_llm_failure_yields_all_fallbacks_without_panic() {
        let client = scripted("definitely not json");
        let content = "正文在此。这是第二句，用来喂抽取式兜底。参考 https://ex.com/a。\
                       下面这些字没有任何信息价值，只是把正文垫过短内容摘要抑制阈值，\
                       好让抽取式兜底这条路径在测试里真的被跑到，而不是被前置规则挡在门外。"
            .to_string();
        let out = AgentPipeline::run(&client, &content);
        assert_eq!(out.category, "其他");
        assert!(out.tags.is_empty());
        assert!(!out.summary.is_empty()); // 抽取式兜底非空
        assert_eq!(out.links.len(), 1); // 链接兜底=扫描结果
        assert!(out.degraded);
    }

    #[test]
    fn empty_content_does_not_panic() {
        let client = scripted("");
        let out = AgentPipeline::run(&client, "");
        assert_eq!(out.category, "其他");
        assert!(out.degraded);
    }
}
