//! Rust→前端事件 payload（02 §0）。命令层与 worker 共用此处定义，保证事件形状单源。

use serde::Serialize;

pub const FRAGMENT_CREATED: &str = "fragment://created";
pub const FRAGMENT_STATUS: &str = "fragment://status";
pub const FRAGMENT_UPDATED: &str = "fragment://updated";
pub const CONFIG_CHANGED: &str = "config://changed";
/// 全局加速键唤起主窗口时，把**当前**剪贴板文本交给输入框当草稿。
/// 注意语义边界：只填草稿，不入库、不进模型上下文——是否收集仍由用户按「收进来」决定
/// （2026-09-26 剪贴板红线：收集必须由明示动作触发）。
pub const CLIPBOARD_DRAFT: &str = "clipboard://draft";
/// 片段已被**物理**删除（区别于 `delete_fragment` 的软删）。前端收到即从 store 移除该 id——
/// 不能再发 `fragment://updated`，那会让前端去 `get_fragment` 一条已不存在的记录、拿 NotFound 当报错弹给用户。
pub const FRAGMENT_PURGED: &str = "fragment://purged";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentCreated {
    pub fragment_id: String,
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentStatus {
    pub fragment_id: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentUpdated {
    pub fragment_id: String,
}

/// `fragment://purged` 载荷：无痕删除后需要前端丢弃的不止一条，但事件按条发（与 updated 一致口径）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentPurged {
    pub fragment_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChanged {
    pub keys: Vec<String>,
}

/// `clipboard://draft` 载荷：`text=None` 表示剪贴板里没有文本（例如只复制了图片），
/// 前端据此给一句诚实的回执，而不是让加速键按下去毫无反应。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardDraft {
    pub text: Option<String>,
}
