//! 大模型使用统计出参（批次17，契约见 02 §7.5）。
//! 只回**计量**，不回任何正文/模型输出；金额一律不在后端算——单价由用户按自己配的端点填（批次18）。

use serde::Serialize;

/// 一个分组桶（按天 / 按任务 / 按模型共用同一形状，前端一套渲染）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    /// 分组键：日期 `2026-09-29`、任务名 `classify`、或模型名 `qwen3-max`。
    pub key: String,
    pub calls: i64,
    pub failures: i64,
    /// 仅对**上游回了 usage** 的调用求和；未知不计入，也不当 0。
    pub tokens_in: i64,
    pub tokens_out: i64,
    /// 本桶里"上游没回 token 数"的调用条数。>0 时前端要写明合计是下限。
    pub tokens_unknown: i64,
    pub avg_latency_ms: i64,
}

/// `get_usage_stats` 出参（02 §7.5）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    /// 统计窗口天数；`0` = 全部历史。
    pub range_days: i64,
    pub total_calls: i64,
    pub total_failures: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_unknown: i64,
    pub avg_latency_ms: i64,
    /// chat 与 embedding 分开计数（embedding 的 token 只有输入方向，混在一起会误读）。
    pub chat_calls: i64,
    pub embedding_calls: i64,
    pub by_day: Vec<UsageBucket>,
    pub by_task: Vec<UsageBucket>,
    pub by_model: Vec<UsageBucket>,
    /// 库里最早一条记录的时刻；`None` = 从未调用过（前端空态要照这个说，不能说"近 N 天无调用"）。
    pub first_recorded_at: Option<String>,
}

