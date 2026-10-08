//! 配置 DTO（02 §4；前端 ipc.ts ConfigDTO）。camelCase 视图，**API Key 永不回传明文**，
//! 仅按 provider 返回 SecretInfo{ hasKey, last4 }。密钥读写走 keyring（每 provider 一把，
//! account=provider 名）；keyring 不可用时回落 `.env`/环境变量（开发期）。
//! 剪贴板**没有**配置项：不做后台监听，收集只由用户主动触发（见 `clip::collect`）。
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, EmbeddingEndpoint, LlmEndpoint, RetryConfig};
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmEndpointConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub timeout_s: u32,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub dim: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryDto {
    pub max_retries: u32,
    pub backoff_base_ms: u64,
    pub backoff_max_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretInfo {
    pub has_key: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last4: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDTO {
    pub llm_primary: LlmEndpointConfig,
    pub llm_fallback: Option<LlmEndpointConfig>,
    pub llm_embedding: EmbeddingConfig,
    pub agent_retry: RetryDto,
    pub agent_auto_retry: bool,
    pub vector_backend: String,
    /// 大模型总开关（app.llm_enabled）：false 时 worker 不领任务、技能/指令拒绝、语义检索退关键词。
    /// 与下面各端点字段独立，关闭不会清空任何已配端点或密钥。
    pub llm_enabled: bool,
    pub autostart: bool,
    pub paste_shortcut: String,
    /// 粘贴图片是否保留原图（批次21-B）：false=超 5 MB 先在 canvas 压一道。
    pub keep_original_image: bool,
    /// 图片收藏目录的**绝对路径**（运行时状态，不落库），前端据此拼
    /// `convertFileSrc(dir + "/" + mediaPath)`。正斜杠形式：Windows 反斜杠进 JSON 要转义，
    /// 且 Rust `Path`/glob 两种分隔符都吃，统一成 `/` 少一处出错面。空=本进程尚未初始化。
    pub media_dir: String,
    /// 用户设定的目录（03 §4 键 `media.dir`）：空串=未自定义，生效目录即默认
    /// `<app_data_dir>/media`。与上一字段分开是必要的：`mediaDir` 每次由磁盘现算、
    /// 回传保存会把"默认"钉死成一条绝对路径（换机器即失效），而"恢复默认"这个动作
    /// 也只有在这个字段上才表达得出来。
    pub media_dir_custom: String,
    /// 媒体目录已占用字节与图片张数（设置「系统」分组显示"占了多少地方"）。
    pub media_usage_bytes: u64,
    pub media_image_count: u64,
    /// 加速键**注册**失败的简述（OS 占用等）。None=已注册或未知；配置存了键但按了没反应时，
    /// 设置「系统」分组就地明示（运行时状态，不落库）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub paste_shortcut_error: Option<String>,
    pub api_keys: HashMap<String, SecretInfo>,
}

fn ep_to_dto(e: &LlmEndpoint) -> LlmEndpointConfig {
    LlmEndpointConfig {
        provider: e.provider.clone(),
        base_url: e.base_url.clone(),
        model: e.model.clone(),
        timeout_s: e.timeout_s,
        max_tokens: e.max_tokens,
    }
}

impl ConfigDTO {
    /// 由 AppConfig 生成前端视图。密钥状态按 provider 从 keyring（回落 .env）计算，只暴露 hasKey/last4。
    pub fn from_config(cfg: &AppConfig) -> Self {
        let mut providers: Vec<&str> = vec![cfg.llm_primary.provider.as_str()];
        if let Some(fb) = &cfg.llm_fallback {
            providers.push(fb.provider.as_str());
        }
        providers.push(cfg.llm_embedding.provider.as_str());
        let mut api_keys: HashMap<String, SecretInfo> = HashMap::new();
        for p in providers {
            let p = p.trim();
            if p.is_empty() || api_keys.contains_key(p) {
                continue;
            }
            api_keys.insert(
                p.to_string(),
                match crate::config::secrets::api_key_for(p) {
                    Some(k) => SecretInfo {
                        has_key: true,
                        last4: Some(last4(&k)),
                    },
                    None => SecretInfo {
                        has_key: false,
                        last4: None,
                    },
                },
            );
        }
        Self {
            llm_primary: ep_to_dto(&cfg.llm_primary),
            llm_fallback: cfg.llm_fallback.as_ref().map(ep_to_dto),
            llm_embedding: EmbeddingConfig {
                provider: cfg.llm_embedding.provider.clone(),
                base_url: cfg.llm_embedding.base_url.clone(),
                model: cfg.llm_embedding.model.clone(),
                dim: cfg.llm_embedding.dim,
            },
            agent_retry: RetryDto {
                max_retries: cfg.agent_retry.max_retries,
                backoff_base_ms: cfg.agent_retry.backoff_base_ms,
                backoff_max_ms: cfg.agent_retry.backoff_max_ms,
            },
            agent_auto_retry: cfg.agent_auto_retry,
            vector_backend: cfg.vector_backend.clone(),
            llm_enabled: cfg.llm_enabled,
            autostart: cfg.autostart,
            paste_shortcut: cfg.paste_shortcut.clone(),
            keep_original_image: cfg.keep_original_image,
            // 媒体目录与占用是进程/磁盘状态，由 command 层（get_config/update_config）补齐——
            // from_config 保持纯函数，可脱离 Tauri 与文件系统单测。
            media_dir: String::new(),
            media_dir_custom: cfg.media_dir.clone(),
            media_usage_bytes: 0,
            media_image_count: 0,
            paste_shortcut_error: None,
            api_keys,
        }
    }
}

/// 取密钥尾 4 位（按字符，避免多字节截断）。
fn last4(secret: &str) -> String {
    let cs: Vec<char> = secret.chars().collect();
    let n = cs.len();
    cs[n.saturating_sub(4)..].iter().collect()
}

/// update_config 入参（02 §4.2 Partial<ConfigDTO>）。各字段可选，仅提供者被合并；
/// 密钥走 `secrets.api_keys`（provider→明文，写 keyring；空串=删除该 provider 密钥），不进 AppConfig。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecretPatch {
    #[serde(default)]
    pub api_keys: Option<HashMap<String, String>>,
}

/// 区分「键缺省」与「键=null」：仅在键存在时被 serde 调用。
/// null → Some(None)（清除），值 → Some(Some(v))（设置）；缺省由 #[serde(default)] 兜成 None（保留）。
mod opt_opt {
    use serde::{Deserialize, Deserializer};

    #[allow(clippy::option_option)]
    pub fn deserialize<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(d).map(Option::Some)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPatch {
    #[serde(default)]
    pub llm_primary: Option<LlmEndpointConfig>,
    // 普通派生无法区分「缺省」与「null」（都塌成 None），故用 deserialize_with：
    // 该函数仅在键存在时被调用，缺失时走 #[serde(default)]。
    #[serde(default, deserialize_with = "opt_opt::deserialize")]
    pub llm_fallback: Option<Option<LlmEndpointConfig>>,
    #[serde(default)]
    pub llm_embedding: Option<EmbeddingConfig>,
    #[serde(default)]
    pub agent_retry: Option<RetryDto>,
    #[serde(default)]
    pub agent_auto_retry: Option<bool>,
    #[serde(default)]
    pub vector_backend: Option<String>,
    /// 大模型总开关（关闭只翻这一枚布尔，绝不清空其它端点配置/密钥）。
    #[serde(default)]
    pub llm_enabled: Option<bool>,
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub paste_shortcut: Option<String>,
    #[serde(default)]
    pub keep_original_image: Option<bool>,
    /// 改存放目录（批次22-A）。空串/仅空白 = 恢复默认目录。
    /// 副作用（搬文件 + 放行 asset scope）在 command 层，见 `commands::settings::apply_media_dir`。
    #[serde(default)]
    pub media_dir_custom: Option<String>,
    #[serde(default)]
    pub secrets: Option<SecretPatch>,
}

fn dto_to_ep(d: &LlmEndpointConfig) -> LlmEndpoint {
    LlmEndpoint {
        provider: d.provider.clone(),
        base_url: d.base_url.clone(),
        model: d.model.clone(),
        timeout_s: d.timeout_s,
        max_tokens: d.max_tokens,
    }
}

impl ConfigPatch {
    /// 字段级校验（02 §4.2）：只检查本次 patch 提供的字段，不要求整份配置已完整
    /// （默认配置是"未配置"态，全量 validate 会误伤局部编辑）。
    pub fn validate(&self) -> Result<(), AppError> {
        if let Some(p) = &self.llm_primary {
            validate_endpoint(p, "llm.primary")?;
        }
        if let Some(Some(fb)) = &self.llm_fallback {
            validate_endpoint(fb, "llm.fallback")?;
        }
        if let Some(e) = &self.llm_embedding {
            if e.model.trim().is_empty() || e.dim == 0 {
                return Err(AppError::InputInvalid("llm.embedding 未配置完整".into()));
            }
        }
        if let Some(r) = &self.agent_retry {
            if r.max_retries > 5 {
                return Err(AppError::InputInvalid("重试次数需在 0–5".into()));
            }
        }
        if let Some(vb) = &self.vector_backend {
            if !matches!(vb.as_str(), "sqlite_vec" | "brute") {
                return Err(AppError::InputInvalid(format!("未知向量后端: {vb}")));
            }
        }
        if let Some(sc) = &self.paste_shortcut {
            crate::config::validate_accelerator(sc)?;
        }
        if let Some(d) = &self.media_dir_custom {
            let d = d.trim();
            // 空 = 恢复默认，合法。非空必须是绝对路径：相对路径会跟着进程的工作目录漂移，
            // 那等于把用户的图片存到一个他不知道在哪、下次启动也可能不是的地方。
            if !d.is_empty() && !std::path::Path::new(d).is_absolute() {
                return Err(AppError::InputInvalid("图片存放目录必须是绝对路径".into()));
            }
        }
        Ok(())
    }
}

fn validate_endpoint(e: &LlmEndpointConfig, name: &str) -> Result<(), AppError> {
    if e.model.trim().is_empty() {
        return Err(AppError::InputInvalid(format!("{name}: model 不能为空")));
    }
    if !e.base_url.starts_with("http://") && !e.base_url.starts_with("https://") {
        return Err(AppError::InputInvalid(format!("{name}: base_url 必须为 http(s) 地址")));
    }
    if !(1..=300).contains(&e.timeout_s) {
        return Err(AppError::InputInvalid(format!("{name}: 超时需在 1–300 秒")));
    }
    if e.max_tokens == 0 {
        return Err(AppError::InputInvalid(format!("{name}: max_tokens 需大于 0")));
    }
    Ok(())
}

impl ConfigPatch {
    /// 把补丁合并进现有配置，产出新 AppConfig（供 save）。
    pub fn apply_to(&self, base: &AppConfig) -> AppConfig {
        let mut c = base.clone();
        if let Some(p) = &self.llm_primary {
            c.llm_primary = dto_to_ep(p);
        }
        if let Some(fb) = &self.llm_fallback {
            c.llm_fallback = fb.as_ref().map(dto_to_ep);
        }
        if let Some(e) = &self.llm_embedding {
            c.llm_embedding = EmbeddingEndpoint {
                provider: e.provider.clone(),
                base_url: e.base_url.clone(),
                model: e.model.clone(),
                dim: e.dim,
            };
        }
        if let Some(r) = &self.agent_retry {
            c.agent_retry = RetryConfig {
                max_retries: r.max_retries,
                backoff_base_ms: r.backoff_base_ms,
                backoff_max_ms: r.backoff_max_ms,
            };
        }
        if let Some(v) = self.agent_auto_retry {
            c.agent_auto_retry = v;
        }
        if let Some(vb) = &self.vector_backend {
            c.vector_backend = vb.clone();
        }
        if let Some(v) = self.llm_enabled {
            c.llm_enabled = v;
        }
        if let Some(v) = self.autostart {
            c.autostart = v;
        }
        if let Some(sc) = &self.paste_shortcut {
            c.paste_shortcut = sc.trim().to_string();
        }
        if let Some(v) = self.keep_original_image {
            c.keep_original_image = v;
        }
        if let Some(d) = &self.media_dir_custom {
            c.media_dir = d.trim().to_string();
        }
        c
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTestResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dto_is_camelcase_and_hides_key() {
        let dto = ConfigDTO::from_config(&AppConfig::default());
        let j = serde_json::to_string(&dto).unwrap();
        assert!(j.contains("\"llmPrimary\"") && j.contains("\"agentAutoRetry\""));
        // 默认配置 provider=openai_compat，测试环境无该密钥 → hasKey:false，且无明文密钥字段。
        assert!(j.contains("\"apiKeys\""), "{j}");
        assert!(j.contains("\"hasKey\":false"), "{j}");
        assert!(!j.contains("last4"), "{j}"); // 未配置时 last4 省略
    }

    // 保留原图开关（批次21-B）：缺省=保留旧值，给定=覆盖；它只影响"以后收的图"，故无副作用校验。
    #[test]
    fn keep_original_image_patch_merges_only_when_given() {
        let base = AppConfig::default();
        assert!(!base.keep_original_image);
        let on: ConfigPatch = serde_json::from_str(r#"{"keepOriginalImage":true}"#).unwrap();
        on.validate().unwrap();
        assert!(on.apply_to(&base).keep_original_image);
        let other: ConfigPatch = serde_json::from_str(r#"{"vectorBackend":"brute"}"#).unwrap();
        assert!(!other.apply_to(&base).keep_original_image);
    }

    // 总开关（app.llm_enabled）：关闭只翻布尔，绝不清空任何已配端点/模型；缺省保留、给定覆盖、可再开。
    #[test]
    fn llm_enabled_patch_flips_flag_without_touching_endpoints() {
        let mut base = AppConfig::default();
        base.llm_primary.model = "deepseek-chat".into();
        base.llm_primary.base_url = "https://api.deepseek.com/v1".into();
        assert!(base.llm_enabled, "默认开");

        // 关闭：只有布尔翻转，端点原样留着（这就是"随时打开即用"的保证）。
        let off: ConfigPatch = serde_json::from_str(r#"{"llmEnabled":false}"#).unwrap();
        off.validate().unwrap();
        let after = off.apply_to(&base);
        assert!(!after.llm_enabled);
        assert_eq!(after.llm_primary.model, base.llm_primary.model);
        assert_eq!(after.llm_primary.base_url, base.llm_primary.base_url);

        // 缺省即保留旧值（改别的字段不该顺手重置总开关）。
        let other: ConfigPatch = serde_json::from_str(r#"{"vectorBackend":"brute"}"#).unwrap();
        assert!(other.apply_to(&base).llm_enabled);
        // 再开：即刻恢复可用。
        let on: ConfigPatch = serde_json::from_str(r#"{"llmEnabled":true}"#).unwrap();
        let mut still_off = base.clone();
        still_off.llm_enabled = false;
        assert!(on.apply_to(&still_off).llm_enabled);
    }

    // 存放目录（批次22-A）：空串=恢复默认（合法）；相对路径挡下——配置不许存一条会随 cwd 漂移的路径。
    #[test]
    fn media_dir_patch_requires_absolute_or_reset() {
        let base = AppConfig::default();
        assert!(base.media_dir.is_empty(), "默认未自定义目录");
        let rel: ConfigPatch = serde_json::from_str(r#"{"mediaDirCustom":"media/pics"}"#).unwrap();
        assert!(matches!(rel.validate(), Err(AppError::InputInvalid(_))));
        let reset: ConfigPatch = serde_json::from_str(r#"{"mediaDirCustom":"  "}"#).unwrap();
        reset.validate().unwrap();
        assert!(reset.apply_to(&base).media_dir.is_empty(), "空白 trim 后恢复默认");
        let abs = std::env::temp_dir().to_string_lossy().to_string();
        let set: ConfigPatch =
            serde_json::from_str(&format!(r#"{{"mediaDirCustom":"{}"}}"#, abs.replace('\\', "\\\\")))
                .unwrap();
        set.validate().unwrap();
        assert_eq!(set.apply_to(&base).media_dir, abs);
    }

    #[test]
    fn last4_takes_tail_chars() {
        assert_eq!(last4("sk-abcdef1234"), "1234");
        assert_eq!(last4("ab"), "ab");
        assert_eq!(last4(""), "");
    }

    // 「停用备用模型」需经 JSON 可表达：null=清除、缺省=保留、对象=设置（三态可区分）。
    #[test]
    fn llm_fallback_three_states_are_distinguishable() {
        let absent: ConfigPatch = serde_json::from_str("{}").unwrap();
        let null: ConfigPatch = serde_json::from_str(r#"{"llmFallback":null}"#).unwrap();
        let set: ConfigPatch = serde_json::from_str(
            r#"{"llmFallback":{"provider":"x","baseUrl":"https://a","model":"m","timeoutS":60,"maxTokens":100}}"#,
        )
        .unwrap();
        assert_eq!(absent.llm_fallback, None); // 缺省 → 保留旧值
        assert_eq!(null.llm_fallback, Some(None)); // null → 清除
        assert!(matches!(set.llm_fallback, Some(Some(_)))); // 对象 → 设置

        // 端到端：在已有备用模型的基础上，null 回执应清空、缺省应保留。
        let mut base = AppConfig::default();
        base.llm_fallback = Some(LlmEndpoint {
            provider: "p".into(),
            base_url: "https://b".into(),
            model: "m".into(),
            timeout_s: 60,
            max_tokens: 100,
        });
        assert!(null.apply_to(&base).llm_fallback.is_none());
        assert!(absent.apply_to(&base).llm_fallback.is_some());
    }

    #[test]
    fn secret_patch_deserializes_camel_api_keys() {
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"secrets":{"apiKeys":{"deepseek":"sk-x","qwen":""}}}"#).unwrap();
        let keys = patch.secrets.unwrap().api_keys.unwrap();
        assert_eq!(keys.get("deepseek").map(String::as_str), Some("sk-x"));
        assert_eq!(keys.get("qwen").map(String::as_str), Some("")); // 空串=删除约定
    }

    #[test]
    fn patch_merges_only_provided_fields() {
        let base = AppConfig::default();
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"vectorBackend":"brute","agentAutoRetry":false}"#).unwrap();
        let merged = patch.apply_to(&base);
        assert_eq!(merged.vector_backend, "brute");
        assert!(!merged.agent_auto_retry);
        // 未提供字段保持
        assert_eq!(merged.llm_primary.provider, base.llm_primary.provider);
        assert_eq!(merged.agent_retry.max_retries, base.agent_retry.max_retries);
    }

    // 加速键可经 patch 改：格式非法当场报错，合法则合并进配置（前端只发改动过的值）。
    #[test]
    fn paste_shortcut_patch_validates_and_merges() {
        let base = AppConfig::default();
        assert_eq!(base.paste_shortcut, crate::config::DEFAULT_PASTE_SHORTCUT);

        let bad: ConfigPatch = serde_json::from_str(r#"{"pasteShortcut":"K"}"#).unwrap();
        assert!(matches!(bad.validate(), Err(AppError::InputInvalid(_))));

        let ok: ConfigPatch = serde_json::from_str(r#"{"pasteShortcut":"Ctrl+Alt+J"}"#).unwrap();
        ok.validate().unwrap();
        assert_eq!(ok.apply_to(&base).paste_shortcut, "Ctrl+Alt+J");

        // 缺省即保留旧键位（改别的字段不该顺手重置加速键）。
        let other: ConfigPatch = serde_json::from_str(r#"{"vectorBackend":"brute"}"#).unwrap();
        assert_eq!(other.apply_to(&base).paste_shortcut, base.paste_shortcut);
    }

    #[test]
    fn merged_patch_respects_validation_bounds() {
        let base = AppConfig::placeholder();
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"agentRetry":{"maxRetries":9,"backoffBaseMs":1,"backoffMaxMs":2}}"#).unwrap();
        let merged = patch.apply_to(&base);
        assert!(merged.validate().is_err()); // 越界重试被 validate 拦下
    }
}
