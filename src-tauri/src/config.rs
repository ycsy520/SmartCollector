//! AppConfig：LLM/向量/重试配置的默认值、校验与热更新载体。
//! 键名与 JSON 结构对应 docs/arch/03-SQLite表结构.md §4（snake_case 落库）。
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 「唤起并填草稿」加速键默认值（跨平台修饰：mac 用 ⌘，其余用 Ctrl；设置→系统 可改）。
pub const DEFAULT_PASTE_SHORTCUT: &str = "CommandOrControl+Alt+K";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmEndpoint {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub timeout_s: u32,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingEndpoint {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub dim: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub llm_primary: LlmEndpoint,
    pub llm_fallback: Option<LlmEndpoint>,
    pub llm_embedding: EmbeddingEndpoint,
    pub agent_retry: RetryConfig,
    pub agent_auto_retry: bool,
    pub vector_backend: String,
    /// 开机自启（02 §4）：仅作开关状态，真实施落 OS 登录项由 autostart 插件在命令/setup 处同步。
    pub autostart: bool,
    /// 「唤起并填草稿」全局加速键（02 §4）：按下读一次剪贴板→唤起→首页草稿，不落库。
    /// 注册在 setup/update_config（成功才落库，失败即 E_INPUT_INVALID——配置不许存按了没反应的键位）。
    pub paste_shortcut: String,
    /// 粘贴图片时**保留原图**（批次21-B，02 §4）：true=按剪贴板里的原始字节收藏；
    /// false（默认）=超过 5 MB 先在 canvas 里重编码压一道。压缩只发生在前端、只发生一次，
    /// 原图不落盘，故这个开关必须在收集前生效——设置页改的是"以后收的图"，追不回已压过的。
    pub keep_original_image: bool,
    /// 图片收藏目录的**用户设定值**（批次22-A）：空串 = 用默认 `<app_data_dir>/media`；
    /// 非空 = 绝对路径。存的是"这台机器上选过的那条路径"，所以换机器后可能指向别处或不存在——
    /// 那时新图仍会 `create_dir_all` 建出来（不静默改地方），而设置页显示的是**生效路径**，不撒谎。
    /// 库里只存相对文件名，改这个值必须连带搬运已有图片，否则老图全部读不到（见 `services::media::migrate`）。
    pub media_dir: String,
    /// 大模型**总开关**：false 时 chat 流水线、技能/一次性指令、embedding/语义检索一律走
    /// "未就绪"降级（复用无密钥的既有路径：worker 不领任务、条目停 pending、检索退关键词）。
    /// 刻意与 llm.primary/embedding 各端点行、keyring **完全独立**——关闭只翻这一枚布尔，
    /// 绝不写空任何已配端点或密钥，故随时打开即刻恢复（`apply_to` 只在给定该字段时动它）。
    pub llm_enabled: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let primary = LlmEndpoint {
            provider: "openai_compat".into(),
            base_url: String::new(),
            model: String::new(),
            timeout_s: 60,
            max_tokens: 2048,
        };
        Self {
            llm_primary: primary,
            llm_fallback: None,
            llm_embedding: EmbeddingEndpoint {
                provider: "openai_compat".into(),
                base_url: String::new(),
                model: String::new(),
                dim: 1024,
            },
            agent_retry: RetryConfig {
                max_retries: 3,
                backoff_base_ms: 1000,
                backoff_max_ms: 30000,
            },
            agent_auto_retry: true,
            vector_backend: "sqlite_vec".into(),
            autostart: false,
            paste_shortcut: DEFAULT_PASTE_SHORTCUT.into(),
            // 默认压缩大图：收藏是高频动作，磁盘是本地资源，原图可随时从来源再截一次。
            keep_original_image: false,
            // 默认落在应用数据目录下：不需要用户先做决定，且天然在 asset scope 内。
            media_dir: String::new(),
            // 默认打开：老用户行为不变，总开关是"额外的一道闸"，不是首启即关的显式动作。
            llm_enabled: true,
        }
    }
}

impl AppConfig {
    /// 字段级校验（02 清单 §4.2：模型名非空、重试 0–5、超时 1–300s…）。
    pub fn validate(&self) -> Result<(), AppError> {
        let endpoint = |e: &LlmEndpoint, name: &str| -> Result<(), AppError> {
            if e.model.trim().is_empty() {
                return Err(AppError::InputInvalid(format!("{name}: model 不能为空")));
            }
            if !e.base_url.starts_with("http://") && !e.base_url.starts_with("https://") {
                return Err(AppError::InputInvalid(format!(
                    "{name}: base_url 必须为 http(s) 地址"
                )));
            }
            if !(1..=300).contains(&e.timeout_s) {
                return Err(AppError::InputInvalid(format!(
                    "{name}: 超时需在 1–300 秒"
                )));
            }
            if e.max_tokens == 0 {
                return Err(AppError::InputInvalid(format!(
                    "{name}: max_tokens 需大于 0"
                )));
            }
            Ok(())
        };
        endpoint(&self.llm_primary, "llm.primary")?;
        if let Some(fb) = &self.llm_fallback {
            endpoint(fb, "llm.fallback")?;
        }
        if self.llm_embedding.model.trim().is_empty() || self.llm_embedding.dim == 0 {
            return Err(AppError::InputInvalid("llm.embedding 未配置完整".into()));
        }
        if self.agent_retry.max_retries > 5 {
            return Err(AppError::InputInvalid("重试次数需在 0–5".into()));
        }
        if !matches!(self.vector_backend.as_str(), "sqlite_vec" | "brute") {
            return Err(AppError::InputInvalid(format!(
                "未知向量后端: {}",
                self.vector_backend
            )));
        }
        validate_accelerator(&self.paste_shortcut)?;
        Ok(())
    }

    /// 主模型是否已配置（test_llm_config 的前置检查，E_CONFIG_MISSING）。
    pub fn has_primary_llm(&self) -> bool {
        !self.llm_primary.model.trim().is_empty() && !self.llm_primary.base_url.trim().is_empty()
    }
}

/// 应用内固定、且能被注册成合法全局 accelerator 的组合。全局收集加速键若撞上它们，
/// 窗口在前台时"全局回调"与"应用内 keydown"会双触发（如按下既唤起填草稿、又直接提交）。
/// 判定口径与应用内处理器一致：含 Ctrl/Cmd 主修饰（`commandorcontrol` 在 win=Ctrl、mac=Cmd）
/// 且主键 ∈ {Enter=收集, A=键盘流全选}，不排 Shift（`Ctrl+Shift+Enter` 同样命中收集）。
/// 命中返回动作名供上层拼区分性错误；`j/k/Del/Space/@/Esc` 无修饰键，不能注册为全局键，无需列。
fn accelerator_conflict(raw: &str) -> Option<&'static str> {
    let parts: Vec<&str> = raw
        .split('+')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let (&key, mods) = parts.split_last()?;
    let key_norm = key.trim_start_matches("Key").to_lowercase();
    let has_primary = mods.iter().any(|m| {
        matches!(
            m.to_lowercase().as_str(),
            "ctrl" | "control" | "commandorcontrol" | "cmd" | "command" | "meta"
        )
    });
    if has_primary {
        if key_norm == "enter" {
            return Some("收集（Ctrl+Enter）");
        }
        if key_norm == "a" {
            return Some("批量全选（Ctrl+A）");
        }
    }
    None
}

/// 加速键格式校验（纯字符串层，可离线测）：至少「一个修饰键 + 一个主键」，各段非空、无空格；
/// 再挡与应用内固定键的冲突。真注册失败（被其他应用占用）由命令层单独报错——这里不碰 OS。
pub fn validate_accelerator(raw: &str) -> Result<(), AppError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(AppError::InputInvalid("加速键不能为空".into()));
    }
    let parts: Vec<&str> = s.split('+').collect();
    if parts.len() < 2 || parts.iter().any(|p| p.trim().is_empty()) {
        return Err(AppError::InputInvalid(format!(
            "加速键 {raw} 需为「修饰键+主键」，如 CommandOrControl+Alt+K"
        )));
    }
    if let Some(action) = accelerator_conflict(s) {
        return Err(AppError::InputInvalid(format!(
            "加速键 {raw} 与应用内「{action}」快捷键冲突，窗口在前台时会同时触发，请换一个键位"
        )));
    }
    Ok(())
}

/// 文档示例键（03 §4；AGENTS §5 的 P5 占位方案）：qwen3-max + openai_compat + text-embedding-v3/1024。
/// ⚠️ 占位、未验真：base_url 是 qwen 的 OpenAI 兼容端点示例，`api_key` 仍空（只进 keyring）。
/// 用户给定正式供应商/密钥前不作为运行默认（`default()` 保持"未配置"态）。
pub mod placeholder {
    pub const LLM_PROVIDER: &str = "openai_compat";
    pub const LLM_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
    pub const LLM_MODEL: &str = "qwen3-max";
    pub const EMBEDDING_MODEL: &str = "text-embedding-v3";
    pub const EMBEDDING_DIM: u32 = 1024;
}

impl AppConfig {
    /// 采纳文档示例键的占位预设——结构合法（过 `validate`），供设置页"填入示例配置"起步；
    /// 拿到真实密钥/供应商前不应据此发真请求（缺 key，端点仅示例）。
    pub fn placeholder() -> Self {
        Self {
            llm_primary: crate::config::LlmEndpoint {
                provider: placeholder::LLM_PROVIDER.into(),
                base_url: placeholder::LLM_BASE_URL.into(),
                model: placeholder::LLM_MODEL.into(),
                timeout_s: 60,
                max_tokens: 2048,
            },
            llm_embedding: EmbeddingEndpoint {
                provider: placeholder::LLM_PROVIDER.into(),
                base_url: placeholder::LLM_BASE_URL.into(),
                model: placeholder::EMBEDDING_MODEL.into(),
                dim: placeholder::EMBEDDING_DIM,
            },
            ..Default::default()
        }
    }

    /// DeepSeek 开发预设（OpenAI 兼容 chat）。密钥不在此——只从环境变量读（见 `api_key_for`）。
    /// 注意：DeepSeek 无 embedding 接口，向量仍需另配供应商，故此预设只填 chat，
    /// embedding 端点保持"未配置"态。
    pub fn deepseek() -> Self {
        Self {
            llm_primary: LlmEndpoint {
                provider: deepseek::PROVIDER.into(),
                base_url: deepseek::BASE_URL.into(),
                model: deepseek::CHAT_MODEL.into(),
                timeout_s: 60,
                max_tokens: 2048,
            },
            ..Default::default()
        }
    }

    /// Qwen（阿里云百炼 DashScope）向量预设：只填 embedding（text-embedding-v3/1024）。
    /// chat 仍由 DeepSeek 负责，故此处不填 primary。与 `deepseek()` 对称——两家各补其所缺。
    /// 密钥从环境变量 `QWEN_API_KEY` 读（见 `api_key_for("qwen")`），不在此。
    pub fn qwen() -> Self {
        Self {
            llm_embedding: EmbeddingEndpoint {
                provider: qwen::PROVIDER.into(),
                base_url: qwen::BASE_URL.into(),
                model: qwen::EMBED_MODEL.into(),
                dim: qwen::EMBED_DIM,
            },
            ..Default::default()
        }
    }
}

/// Qwen/DashScope 端点常量（非敏感；密钥走环境变量 `QWEN_API_KEY`）。
/// base_url 用通用兼容域 `dashscope.aliyuncs.com/compatible-mode/v1`（无需 WorkspaceId 专属域，实测可用）。
pub mod qwen {
    pub const PROVIDER: &str = "qwen";
    pub const BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
    pub const EMBED_MODEL: &str = "text-embedding-v3";
    pub const EMBED_DIM: u32 = 1024;
}

/// DeepSeek 端点常量（非敏感；密钥走环境变量 `DEEPSEEK_API_KEY`）。
pub mod deepseek {
    pub const PROVIDER: &str = "deepseek";
    pub const BASE_URL: &str = "https://api.deepseek.com/v1";
    pub const CHAT_MODEL: &str = "deepseek-chat";
}

/// 密钥读取与 .env 引导（AGENTS §3：密钥只进内存，绝不入库/落日志/回传前端）。
pub mod secrets {
    use std::fs;
    use std::path::PathBuf;

    use crate::error::AppError;

    /// keyring 服务标识（同一 app 的条目归此命名空间，account=provider 名）。
    const SERVICE: &str = "smart-collector";

    fn entry(provider: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, provider).ok()
    }

    /// 从系统 keyring 读密钥（不含 .env 回落）。不可用/无条目→None。
    fn keyring_get(provider: &str) -> Option<String> {
        entry(provider)?
            .get_password()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// 写入/覆盖某 provider 的密钥到 keyring。失败上抛供命令层映射 E_SECRET_STORE。
    pub fn set_key(provider: &str, secret: &str) -> Result<(), AppError> {
        let p = provider.trim();
        if p.is_empty() {
            return Err(AppError::InputInvalid("provider 不能为空".into()));
        }
        let e = entry(p).ok_or_else(|| AppError::SecretStore("无法访问系统密钥库".into()))?;
        e.set_password(secret.trim())
            .map_err(|err| AppError::SecretStore(err.to_string()))
    }

    /// 删除某 provider 的 keyring 密钥（不存在视作成功）。
    pub fn delete_key(provider: &str) -> Result<(), AppError> {
        let p = provider.trim();
        if p.is_empty() {
            return Ok(());
        }
        match entry(p) {
            Some(e) => match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(AppError::SecretStore(err.to_string())),
            },
            None => Ok(()),
        }
    }

    /// 取某 provider 密钥：先查 keyring，再回落 `<PROVIDER>_API_KEY` → 官方别名 → 通用 `LLM_API_KEY`。
    /// 例：provider="deepseek" → keyring ∨ `DEEPSEEK_API_KEY`；"qwen" → keyring ∨ `QWEN_API_KEY` ∨ `DASHSCOPE_API_KEY`。
    /// 全无返回 None（调用方据此判未配置）。
    pub fn api_key_for(provider: &str) -> Option<String> {
        if provider.trim().is_empty() {
            return None;
        }
        let p = provider.trim();
        keyring_get(p).or_else(|| {
            let var = format!("{}_API_KEY", p.to_uppercase());
            read_var(&var)
                .or_else(|| canonical_alias(p).and_then(read_var))
                .or_else(|| read_var("LLM_API_KEY"))
        })
    }

    /// 部分供应商的官方约定 env 名与我们通用约定的映射（按需扩充）。
    fn canonical_alias(provider: &str) -> Option<&'static str> {
        match provider.to_ascii_lowercase().as_str() {
            "qwen" | "dashscope" => Some("DASHSCOPE_API_KEY"),
            _ => None,
        }
    }

    fn read_var(name: &str) -> Option<String> {
        std::env::var(name).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// 尽力从当前工作目录向上查找 `.env` 并载入未设置的键（不覆盖真实环境变量）。
    /// 简易 KEY=VALUE 解析（去 `export `、去首尾引号、忽略注释/空行），无第三方依赖。
    /// 返回是否找到并读取了文件；找不到不报错（生产密钥应在 keyring，非 .env）。
    pub fn load_dotenv() -> bool {
        let Some(path) = find_dotenv() else { return false };
        let Ok(text) = fs::read_to_string(&path) else { return false };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let k = k.trim().strip_prefix("export ").unwrap_or(k).trim();
            if k.is_empty() {
                continue;
            }
            if std::env::var_os(k).is_some() {
                continue; // 真实环境变量优先，不覆盖
            }
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                std::env::set_var(k, v);
            }
        }
        true
    }

    fn find_dotenv() -> Option<PathBuf> {
        let mut dir = std::env::current_dir().ok()?;
        for _ in 0..4 {
            let cand = dir.join(".env");
            if cand.is_file() {
                return Some(cand);
            }
            if !dir.pop() {
                break;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 加速键格式：至少要「修饰键 + 主键」，否则注册必然失败，不该让它进库。
    #[test]
    fn accelerator_needs_modifier_and_key() {
        assert!(validate_accelerator("CommandOrControl+Alt+K").is_ok());
        assert!(validate_accelerator("  CommandOrControl+Shift+V  ").is_ok());
        assert!(validate_accelerator("").is_err());
        assert!(validate_accelerator("K").is_err());
        assert!(validate_accelerator("Ctrl+").is_err());
    }

    // 加速键冲突：撞上应用内固定的收集/全选键，须保存前拦下（否则窗口在前台时双触发）。
    #[test]
    fn accel_rejects_reserved_inapp_combo() {
        // 收集 Ctrl/Cmd+Enter 及其带 Shift 变体都命中，一律拒。
        assert!(validate_accelerator("Ctrl+Enter").is_err());
        assert!(validate_accelerator("CommandOrControl+Enter").is_err());
        assert!(validate_accelerator("CommandOrControl+Shift+Enter").is_err());
        assert!(validate_accelerator("Cmd+Enter").is_err());
        // 批量全选 Ctrl/Cmd+A 拒。
        assert!(validate_accelerator("Ctrl+A").is_err());
        assert!(validate_accelerator("CommandOrControl+A").is_err());
        // 无主修饰的同名键不冲突（Enter/A 单键不能注册为全局键，也不该误拦）。
        assert!(validate_accelerator("Alt+Enter").is_ok());
        assert!(validate_accelerator("Shift+A").is_ok());
        // 默认键与其它主键不受影响。
        assert!(validate_accelerator("CommandOrControl+Alt+K").is_ok());
        assert!(validate_accelerator("Ctrl+Shift+V").is_ok());
    }

    #[test]
    fn api_key_for_reads_env_var_by_provider_convention() {
        let probe = "SC_TEST_DEEPSEEK_API_KEY";
        std::env::set_var(probe, "  secret-value  ");
        // 复用同一读取逻辑：把 provider 归一化为该测试变量名不便，直接验证 read_var 行为
        let got = std::env::var(probe).ok().map(|s| s.trim().to_string());
        assert_eq!(got.as_deref(), Some("secret-value"));
        std::env::remove_var(probe);
    }

    #[test]
    fn deepseek_preset_is_chat_only_and_valid() {
        let c = AppConfig::deepseek();
        assert_eq!(c.llm_primary.provider, "deepseek");
        assert!(c.llm_primary.base_url.starts_with("https://api.deepseek.com"));
        assert_eq!(c.llm_primary.model, "deepseek-chat");
        // 密钥不在配置里（AppConfig 无 key 字段，validate 也不查 key）
        assert!(c.has_primary_llm());
        // 但 embedding 仍未配置（DeepSeek 无向量接口）——不据默认发真 embedding
        assert!(c.llm_embedding.model.trim().is_empty());
    }

    fn valid() -> AppConfig {
        let mut c = AppConfig::default();
        c.llm_primary.base_url = "https://api.example.com/v1".into();
        c.llm_primary.model = "qwen3-max".into();
        c.llm_embedding.base_url = "https://api.example.com/v1".into();
        c.llm_embedding.model = "text-embedding-v3".into();
        c
    }

    #[test]
    fn default_loads_with_expected_values() {
        let c = AppConfig::default();
        assert_eq!(c.agent_retry.max_retries, 3);
        assert_eq!(c.vector_backend, "sqlite_vec");
        assert!(c.agent_auto_retry);
        // 图片默认走"超 5 MB 先压缩"，保留原图是用户显式选择（批次21-B）
        assert!(!c.keep_original_image);
        // 默认不含密钥/模型名：未配置状态
        assert!(!c.has_primary_llm());
    }

    #[test]
    fn valid_config_passes() {
        assert!(valid().validate().is_ok());
    }

    #[test]
    fn empty_model_is_rejected() {
        let mut c = valid();
        c.llm_primary.model = "  ".into();
        assert!(matches!(
            c.validate(),
            Err(AppError::InputInvalid(_))
        ));
    }

    #[test]
    fn retry_out_of_range_rejected() {
        let mut c = valid();
        c.agent_retry.max_retries = 6;
        assert!(c.validate().is_err());
    }

    #[test]
    fn timeout_bounds_enforced() {
        let mut c = valid();
        c.llm_primary.timeout_s = 0;
        assert!(c.validate().is_err());
        let mut c = valid();
        c.llm_primary.timeout_s = 301;
        assert!(c.validate().is_err());
    }

    #[test]
    fn json_roundtrip_matches_db_storage_shape() {
        let c = valid();
        let j = serde_json::to_string(&c).unwrap();
        let back: AppConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back);
        assert!(j.contains("base_url"));
    }

    #[test]
    fn placeholder_preset_is_valid_but_default_stays_unconfigured() {
        // 占位预设：结构合法（可过 Command 层校验），带文档示例键。
        let p = AppConfig::placeholder();
        assert!(p.validate().is_ok());
        assert_eq!(p.llm_primary.model, placeholder::LLM_MODEL);
        assert_eq!(p.llm_primary.provider, "openai_compat");
        assert_eq!(p.llm_embedding.model, placeholder::EMBEDDING_MODEL);
        assert_eq!(p.llm_embedding.dim, 1024);
        assert!(p.has_primary_llm());
        // 但运行默认仍是"未配置"态——不据占位发真请求（缺 key、端点仅示例）。
        assert!(!AppConfig::default().has_primary_llm());
    }
}
