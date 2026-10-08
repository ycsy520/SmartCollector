//! 统一错误类型。错误码取值以 docs/arch/02-Tauri-Command清单.md §6 为唯一契约源。
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorLevel {
    /// 客户端错误，不重试
    Client,
    /// 临时错误，可重试
    Transient,
    /// 致命错误，需人工介入
    Fatal,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("input empty")]
    InputEmpty,
    #[error("input invalid: {0}")]
    InputInvalid(String),
    #[error("input too large")]
    InputTooLarge,

    #[error("invalid url: {0}")]
    UrlInvalid(String),
    #[error("fetch failed: {0}")]
    FetchFailed(String),

    #[error("clipboard read failed")]
    ClipboardRead,

    #[error("not found")]
    NotFound,
    #[error("state conflict")]
    StateConflict,
    #[error("retry limit reached")]
    RetryLimit,
    /// 隐私闸门（§2.2）：本地规则命中，调用方没带用户明示的外发确认。
    /// `{0}` 只放**类型词**（如"身份证号"），命中的内容片段本身绝不进文案。
    #[error("sensitive content, explicit confirmation required: {0}")]
    SensitiveConfirm(String),

    #[error("db read failed: {0}")]
    DbRead(String),
    #[error("db write failed: {0}")]
    DbWrite(String),

    #[error("config missing")]
    ConfigMissing,
    #[error("secret store failed: {0}")]
    SecretStore(String),

    #[error("llm timeout")]
    LlmTimeout,
    #[error("llm rate limited")]
    LlmRateLimited,
    #[error("llm bad output: {0}")]
    LlmBadOutput(String),

    #[error("vector unavailable: {0}")]
    VectorUnavailable(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::InputEmpty => "E_INPUT_EMPTY",
            AppError::InputInvalid(_) => "E_INPUT_INVALID",
            AppError::InputTooLarge => "E_INPUT_TOO_LARGE",
            AppError::UrlInvalid(_) => "E_URL_INVALID",
            AppError::FetchFailed(_) => "E_FETCH_FAILED",
            AppError::ClipboardRead => "E_CLIPBOARD_READ",
            AppError::NotFound => "E_NOT_FOUND",
            AppError::StateConflict => "E_STATE_CONFLICT",
            AppError::RetryLimit => "E_RETRY_LIMIT",
            AppError::SensitiveConfirm(_) => "E_SENSITIVE_CONFIRM",
            AppError::DbRead(_) => "E_DB_READ",
            AppError::DbWrite(_) => "E_DB_WRITE",
            AppError::ConfigMissing => "E_CONFIG_MISSING",
            AppError::SecretStore(_) => "E_SECRET_STORE",
            AppError::LlmTimeout => "E_LLM_TIMEOUT",
            AppError::LlmRateLimited => "E_LLM_RATE_LIMIT",
            AppError::LlmBadOutput(_) => "E_LLM_BAD_OUTPUT",
            AppError::VectorUnavailable(_) => "E_VECTOR_UNAVAILABLE",
            AppError::Internal(_) => "E_INTERNAL",
        }
    }

    pub fn level(&self) -> ErrorLevel {
        match self {
            AppError::InputEmpty
            | AppError::InputInvalid(_)
            | AppError::InputTooLarge
            | AppError::UrlInvalid(_)
            | AppError::NotFound
            | AppError::StateConflict
            | AppError::RetryLimit
            | AppError::SensitiveConfirm(_)
            | AppError::ConfigMissing => ErrorLevel::Client,

            AppError::FetchFailed(_)
            | AppError::LlmTimeout
            | AppError::LlmRateLimited
            | AppError::LlmBadOutput(_)
            | AppError::VectorUnavailable(_) => ErrorLevel::Transient,

            AppError::ClipboardRead
            | AppError::DbRead(_)
            | AppError::DbWrite(_)
            | AppError::SecretStore(_)
            | AppError::Internal(_) => ErrorLevel::Fatal,
        }
    }

    /// Command 失败时回传给前端的结构化错误（{ code, message }，见 02 清单通用约定）。
    pub fn to_dto(&self) -> ErrorDto {
        ErrorDto {
            code: self.code(),
            message: self.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDto {
    pub code: &'static str,
    pub message: String,
}

/// Command 以 `Result<T, AppError>` 返回时，Tauri 需错误侧可序列化。
/// 序列化成 02 契约的 `{ code, message }`（复用 `to_dto`，camelCase），不外泄内部结构。
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_dto().serialize(serializer)
    }
}

impl From<r2d2::Error> for AppError {
    fn from(e: r2d2::Error) -> Self {
        AppError::DbWrite(e.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::DbWrite(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_contract_02() {
        assert_eq!(AppError::InputEmpty.code(), "E_INPUT_EMPTY");
        assert_eq!(AppError::RetryLimit.code(), "E_RETRY_LIMIT");
        assert_eq!(
            AppError::SensitiveConfirm("身份证号".into()).code(),
            "E_SENSITIVE_CONFIRM"
        );
        assert_eq!(
            AppError::SensitiveConfirm("x".into()).level(),
            ErrorLevel::Client
        );
        assert_eq!(
            AppError::LlmBadOutput("x".into()).code(),
            "E_LLM_BAD_OUTPUT"
        );
        assert_eq!(AppError::Internal("x".into()).code(), "E_INTERNAL");
    }

    #[test]
    fn levels_classify_correctly() {
        assert_eq!(AppError::InputEmpty.level(), ErrorLevel::Client);
        assert_eq!(AppError::LlmTimeout.level(), ErrorLevel::Transient);
        assert_eq!(AppError::DbWrite("x".into()).level(), ErrorLevel::Fatal);
    }

    #[test]
    fn dto_serializes_code_and_message() {
        let v: serde_json::Value =
            serde_json::to_value(AppError::UrlInvalid("ftp://x".into()).to_dto()).unwrap();
        assert_eq!(v["code"], "E_URL_INVALID");
        assert!(v["message"]
            .as_str()
            .unwrap()
            .contains("ftp://x"));
    }
}
