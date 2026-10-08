//! 配置 command（02 §4）。get_config/update_config 纯 DB 可离线；
//! test_llm_config（02 §4.3）经 reqwest 真网络（P5），密钥从 .env/keyring 读、不回传前端。
use rusqlite::Connection;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

use crate::config::{EmbeddingEndpoint, LlmEndpoint};
use crate::db::config_store;
use crate::dto::settings as st;
use crate::error::AppError;
use crate::events::{ConfigChanged, CONFIG_CHANGED};
use crate::state::AppState;

/// 加速键的**运行时**注册状态（不落库）：setup 与 update_config 写入，get_config 读出。
/// 库里的键位可能与 OS 不一致（开机时被别的应用抢走），设置「系统」分组据此明示"按了没反应"。
static SHORTCUT_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_shortcut_error(msg: Option<String>) {
    *SHORTCUT_ERROR.lock().unwrap() = msg;
}

fn shortcut_error() -> Option<String> {
    SHORTCUT_ERROR.lock().unwrap().clone()
}

/// 在 OS 层注册「唤起并填草稿」加速键（重复注册在 Windows 直接报错，故先让出同名字）。
/// 失败上抛 E_INPUT_INVALID——调用方据此不落库，配置里不会留下按了没反应的键位。
pub(crate) fn apply_paste_shortcut(app: &AppHandle, accel: &str) -> Result<(), AppError> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let sc = app.global_shortcut();
    let _ = sc.unregister(accel); // 未注册过时报错，可忽略
    sc.on_shortcut(accel, |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            crate::summon_with_clipboard_draft(app);
        }
    })
    .map_err(|e| AppError::InputInvalid(format!("加速键 {accel} 注册失败（可能已被其他应用占用）：{e}")))
}

/// 把旧键位从 OS 摘掉（改键成功后调用；失败忽略——旧键要么已不被本应用持有，要么重启即清）。
pub(crate) fn release_paste_shortcut(app: &AppHandle, accel: &str) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister(accel);
}

pub(crate) fn get_config_impl(conn: &Connection) -> Result<st::ConfigDTO, AppError> {
    let cfg = config_store::load(conn)?;
    Ok(st::ConfigDTO::from_config(&cfg))
}

/// 补齐媒体目录与占用（运行时/磁盘状态，不落库）。目录未初始化（单测、异常启动）就留着空值——
/// 设置页据此显示"未知"，而不是编一个 0 假装"没占地方"。
fn fill_media(dto: &mut st::ConfigDTO) {
    let Ok(dir) = crate::services::media::media_dir() else { return };
    let (bytes, count) = crate::services::media::dir_usage(&dir);
    dto.media_dir = dir.to_string_lossy().replace('\\', "/");
    dto.media_usage_bytes = bytes;
    dto.media_image_count = count;
}

/// 切换图片存放目录（批次22-A）：搬已有图片 → 换本进程生效目录 → 放行 asset 作用域 → 自检。
///
/// 任一步失败都已就地回滚（图片搬回去、目录改回去），调用方再把配置回滚——配置说一个目录、
/// 磁盘上是另一个目录，是最难排查的那种坏状态。
/// 放行失败时不"放宽 scope 重试"：静态配置只有 `$APPDATA/media/**`，放宽等于把整块磁盘交给页面里的 `<img>`。
fn apply_media_dir(app: &AppHandle, custom: &str) -> Result<(), AppError> {
    let target = crate::services::media::resolve_dir(custom)
        .ok_or_else(|| AppError::Internal("媒体目录尚未初始化".into()))?;
    let old = crate::services::media::media_dir()?;
    if target != old {
        crate::services::media::migrate(&old, &target)?;
        crate::services::media::set_media_dir(target.clone());
    }
    if let Err(e) = allow_and_probe(app, &target) {
        if target != old {
            let _ = crate::services::media::migrate(&target, &old);
            crate::services::media::set_media_dir(old);
        }
        return Err(e);
    }
    Ok(())
}

/// 精确放行这一个目录，并探一次"界面真的读得到"。
/// 探针用**前端实际会发的那种分隔符**（`fill_media` 给的是正斜杠，`lib/media.ts` 直接拼进 URL）：
/// 斜杠方向与 glob 匹配是否等价从未被实测，这里把它变成保存时的断言，而不是留下一张永远加载不出来的图。
fn allow_and_probe(app: &AppHandle, dir: &std::path::Path) -> Result<(), AppError> {
    use tauri::Manager;
    let scope = app.asset_protocol_scope();
    scope
        .allow_directory(dir, true)
        .map_err(|e| AppError::Internal(format!("开放图片目录失败：{e}")))?;
    let probe = dir.join("scope-probe.png").to_string_lossy().replace('\\', "/");
    if scope.is_allowed(std::path::Path::new(&probe)) {
        Ok(())
    } else {
        Err(AppError::Internal(
            "图片目录没能开放给界面（asset 作用域未命中），缩略图会显示不出来".into(),
        ))
    }
}

pub(crate) fn update_config_impl(conn: &Connection, patch: &st::ConfigPatch) -> Result<st::ConfigDTO, AppError> {
    patch.validate()?; // 字段级校验（02 §4.2）：只查本次提供的字段，失败即 E_INPUT_INVALID 不落库
    let base = config_store::load(conn)?;
    let merged = patch.apply_to(&base);
    config_store::save(conn, &merged)?;
    // config://changed 事件由 update_config 命令发出（需 AppHandle）。
    let reloaded = config_store::load(conn)?;
    Ok(st::ConfigDTO::from_config(&reloaded))
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Result<st::ConfigDTO, AppError> {
    let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
    let mut dto = get_config_impl(&conn)?;
    dto.paste_shortcut_error = shortcut_error(); // 运行时状态：键位存着但 OS 没注册成功
    fill_media(&mut dto); // 图片目录与占用同理（磁盘状态）
    Ok(dto)
}

#[tauri::command]
pub fn update_config(
    state: State<'_, AppState>,
    app: AppHandle,
    patch: st::ConfigPatch,
) -> Result<st::ConfigDTO, AppError> {
    patch.validate()?; // 先过格式校验，再动任何副作用（keyring / OS 注册 / DB）
    let keys = changed_keys(&patch);
    // 先落密钥到 keyring（每 provider 一把；空串=删除），失败即中止、不写配置。
    if let Some(map) = patch.secrets.as_ref().and_then(|s| s.api_keys.as_ref()) {
        for (provider, value) in map {
            if value.trim().is_empty() {
                crate::config::secrets::delete_key(provider)?;
            } else {
                crate::config::secrets::set_key(provider, value)?;
            }
        }
    }
    let dto = {
        let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        let base = config_store::load(&conn)?;
        // 存放目录（批次22-A）：**先搬文件与放行作用域，再落库**。顺序反过来的话，落库成功而搬迁
        // 半途失败会留下"配置指着一个没有图片的目录"（重启后满屏占位框）；而现在最坏只是保存失败。
        // apply_media_dir 自身失败时已把图片与生效目录还原，这里直接中止、配置一个字节都没动。
        if let Some(custom) = &patch.media_dir_custom {
            apply_media_dir(&app, custom)?;
        }
        let new_sc = patch.paste_shortcut.as_deref().map(|s| s.trim().to_string());
        // 加速键先注册后落库：注册失败（键位被别的应用占用）就地报错，配置保持原键位——
        // 落库一个按了没反应的键位，比保存失败更难排查。
        if let Some(sc) = &new_sc {
            apply_paste_shortcut(&app, sc)?;
        }
        let dto = match update_config_impl(&conn, &patch) {
            Ok(d) => d,
            Err(e) => {
                // 落库失败：把刚搬的图片与生效目录还原（尽力而为），别让磁盘与配置各说一半。
                if patch.media_dir_custom.is_some() {
                    let _ = apply_media_dir(&app, &base.media_dir);
                }
                return Err(e);
            }
        };
        if let Some(sc) = &new_sc {
            if base.paste_shortcut != *sc {
                release_paste_shortcut(&app, &base.paste_shortcut);
            }
            set_shortcut_error(None);
        }
        dto
    };
    // 返回体与 get_config 同形（含媒体目录/占用与键位注册态）：前端拿这份回执直接覆盖 store，
    // 少一个字段就会在界面上显示成"未知/0"，那是比多查一次磁盘更糟的代价。
    let mut dto = dto;
    dto.paste_shortcut_error = shortcut_error();
    fill_media(&mut dto);
    // 开机自启落 OS 登录项（autostart 插件）：仅在本次 patch 显式改动该开关时同步，失败不阻断配置保存。
    if let Some(enable) = patch.autostart {
        use tauri_plugin_autostart::ManagerExt;
        let al = app.autolaunch();
        let _ = if enable { al.enable() } else { al.disable() };
    }
    // 热更新内存态（当前无跨调用缓存的进程内配置：worker 每次自行 load）→ 只发事件供前端刷新。
    let _ = app.emit(CONFIG_CHANGED, ConfigChanged { keys });
    Ok(dto)
}
/// 本次 patch 实际改动的配置键（02 §4.2 `config://changed {keys}`）。键名与落库 config key 对齐。
fn changed_keys(patch: &st::ConfigPatch) -> Vec<String> {
    let mut k = Vec::new();
    if patch.llm_primary.is_some() {
        k.push("llm_primary".to_string());
    }
    if patch.llm_fallback.is_some() {
        k.push("llm_fallback".to_string());
    }
    if patch.llm_embedding.is_some() {
        k.push("llm_embedding".to_string());
    }
    if patch.agent_retry.is_some() {
        k.push("agent_retry".to_string());
    }
    if patch.agent_auto_retry.is_some() {
        k.push("agent_auto_retry".to_string());
    }
    if patch.vector_backend.is_some() {
        k.push("vector_backend".to_string());
    }
    if patch.llm_enabled.is_some() {
        k.push("llm_enabled".to_string());
    }
    if patch.autostart.is_some() {
        k.push("autostart".to_string());
    }
    if patch.paste_shortcut.is_some() {
        k.push("paste_shortcut".to_string());
    }
    if patch.keep_original_image.is_some() {
        k.push("keep_original_image".to_string());
    }
    if patch.media_dir_custom.is_some() {
        k.push("media_dir".to_string());
    }
    if patch
        .secrets
        .as_ref()
        .and_then(|s| s.api_keys.as_ref())
        .is_some_and(|m| !m.is_empty())
    {
        k.push("secrets".to_string());
    }
    k
}

// 02 §4.3：按 `target`（primary/fallback/embedding，默认 primary）测对应**已保存**端点的连通。
// 仅"目标端点未配置"抛 E_CONFIG_MISSING；缺密钥/网络失败以 ok:false 结构化返回；不因测试改配置。
// 密钥只从 keyring/env 取、不回传明文。blocking 传输放独立 std 线程（避 nested-runtime panic）。
// `api_key`：前端密钥框里的**草稿**（未保存）。有则本次 ping 优先用它，让用户"粘完即测、不必先保存"；
// 绝不落 keyring/DB/日志，只有点「保存」走 update_config 才真正持久化。缺省/空串 → 回落 keyring∨env。
#[tauri::command]
pub fn test_llm_config(
    state: State<'_, AppState>,
    target: Option<String>,
    api_key: Option<String>,
) -> Result<st::LlmTestResult, AppError> {
    let cfg = {
        let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        config_store::load(&conn)?
    };
    match target.as_deref().unwrap_or("primary") {
        "fallback" => {
            let ep = cfg
                .llm_fallback
                .filter(|e| !e.model.trim().is_empty() && !e.base_url.trim().is_empty())
                .ok_or(AppError::ConfigMissing)?;
            run_chat_test(ep, api_key)
        }
        "embedding" => {
            let e = cfg.llm_embedding;
            if e.model.trim().is_empty() || e.base_url.trim().is_empty() {
                return Err(AppError::ConfigMissing);
            }
            run_embedding_test(e, api_key)
        }
        _ => {
            if !cfg.has_primary_llm() {
                return Err(AppError::ConfigMissing); // 唯一对主模型的抛错路径（02 §4.3）
            }
            run_chat_test(cfg.llm_primary.clone(), api_key)
        }
    }
}

fn run_chat_test(ep: LlmEndpoint, draft_key: Option<String>) -> Result<st::LlmTestResult, AppError> {
    let model = ep.model.clone();
    let api_key = match resolve_test_key(draft_key, &ep.provider) {
        Some(k) => k,
        None => return Ok(missing_key(&model, &ep.provider)),
    };
    let handle = std::thread::spawn(move || llm_ping(&ep, &api_key));
    let (ok, latency_ms, err) = join_test(handle);
    Ok(st::LlmTestResult {
        ok,
        latency_ms,
        model: Some(model),
        error_code: err.as_ref().map(|(c, _)| c.clone()),
        message: err.map(|(_, m)| m),
    })
}

fn run_embedding_test(
    ep: EmbeddingEndpoint,
    draft_key: Option<String>,
) -> Result<st::LlmTestResult, AppError> {
    let model = ep.model.clone();
    let api_key = match resolve_test_key(draft_key, &ep.provider) {
        Some(k) => k,
        None => return Ok(missing_key(&model, &ep.provider)),
    };
    let handle = std::thread::spawn(move || embed_ping(ep, &api_key));
    let (ok, latency_ms, err) = join_test(handle);
    Ok(st::LlmTestResult {
        ok,
        latency_ms,
        model: Some(model),
        error_code: err.as_ref().map(|(c, _)| c.clone()),
        message: err.map(|(_, m)| m),
    })
}

// 测试取钥顺序：草稿（非空）优先 → 回落 keyring∨env。草稿绝不落库，仅本次探测用。
fn resolve_test_key(draft_key: Option<String>, provider: &str) -> Option<String> {
    draft_key
        .filter(|k| !k.trim().is_empty())
        .or_else(|| crate::config::secrets::api_key_for(provider))
}

fn missing_key(model: &str, provider: &str) -> st::LlmTestResult {
    st::LlmTestResult {
        ok: false,
        latency_ms: None,
        model: Some(model.to_string()),
        error_code: Some("E_CONFIG_MISSING".into()),
        message: Some(format!(
            "未找到密钥（keyring 或 .env 的 {}_API_KEY）",
            provider.to_uppercase()
        )),
    }
}

fn join_test(
    h: std::thread::JoinHandle<(bool, Option<u64>, Option<(String, String)>)>,
) -> (bool, Option<u64>, Option<(String, String)>) {
    match h.join() {
        Ok(r) => r,
        Err(_) => (false, None, Some(("E_INTERNAL".to_string(), "连接测试线程异常退出".to_string()))),
    }
}

/// 在独立线程内发起一次真 chat 调用并计时。返回 (ok, 延迟ms, 结构化错误码/信息)。
/// 不回传上游原文，只给映射后的错误码与简述（AGENTS §3：不把裸输出透传前端）。
/// **不计入使用统计**（批次17 口径：面板只统计"自动处理与再加工"的调用；测试 ping 走
/// 默认 `NoUsageRecorder`，混进来会让"花了多少"失真）。
fn llm_ping(endpoint: &LlmEndpoint, api_key: &str) -> (bool, Option<u64>, Option<(String, String)>) {
    use crate::llm::openai::{HttpTransport, OpenAiProvider};
    use crate::llm::provider::{ChatMessage, ChatOpts, Provider};
    use std::time::{Duration, Instant};

    let transport = HttpTransport::new();
    let provider = OpenAiProvider::new(&endpoint.base_url, api_key, &transport);
    let opts = ChatOpts {
        model: endpoint.model.clone(),
        timeout: Duration::from_secs(endpoint.timeout_s as u64),
        max_tokens: 16,
        temperature: 0.0,
    };
    let start = Instant::now();
    match provider.chat(&[ChatMessage::user("只回复两个字母：ok")], &opts) {
        Ok(out) => {
            let ms = start.elapsed().as_millis() as u64;
            if out.content.trim().is_empty() {
                (false, Some(ms), Some(("E_LLM_BAD_OUTPUT".into(), "响应为空".into())))
            } else {
                (true, Some(ms), None)
            }
        }
        Err(e) => (false, Some(start.elapsed().as_millis() as u64), Some((e.code().to_string(), e.to_string()))),
    }
}

/// 独立线程内发一次真 embedding 调用并计时；`embed` 内含维度校验（不符→VectorUnavailable）。
fn embed_ping(endpoint: EmbeddingEndpoint, api_key: &str) -> (bool, Option<u64>, Option<(String, String)>) {
    use crate::vector::embedding::{Embedder, HttpEmbeddingTransport, OpenAiEmbedder};
    use std::time::Instant;

    let transport = HttpEmbeddingTransport::new();
    let embedder = OpenAiEmbedder::new(endpoint.clone(), api_key, &transport);
    let start = Instant::now();
    match embedder.embed("连通性测试") {
        Ok(v) => {
            let ms = start.elapsed().as_millis() as u64;
            if v.is_empty() {
                (false, Some(ms), Some(("E_VECTOR_UNAVAILABLE".into(), "返回空向量".into())))
            } else {
                (true, Some(ms), None)
            }
        }
        Err(e) => (false, Some(start.elapsed().as_millis() as u64), Some((e.code().to_string(), e.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_returns_config_with_no_key() {
        let conn = crate::db::test_conn();
        config_store::ensure_defaults(&conn, &crate::config::AppConfig::default()).unwrap();
        let dto = get_config_impl(&conn).unwrap();
        // 默认 provider=openai_compat，测试环境无密钥 → 该 provider hasKey:false（map 存在但无明文）。
        assert!(dto.api_keys.values().all(|s| !s.has_key));
        assert_eq!(dto.vector_backend, "sqlite_vec");
    }

    #[test]
    fn update_validates_before_persisting() {
        let conn = crate::db::test_conn();
        config_store::ensure_defaults(&conn, &crate::config::AppConfig::default()).unwrap();
        // 合法：切后端
        let patch: st::ConfigPatch = serde_json::from_str(r#"{"vectorBackend":"brute"}"#).unwrap();
        let dto = update_config_impl(&conn, &patch).unwrap();
        assert_eq!(dto.vector_backend, "brute");
        // 非法：越界重试 → 报错且不落库
        let bad: st::ConfigPatch =
            serde_json::from_str(r#"{"agentRetry":{"maxRetries":9,"backoffBaseMs":1,"backoffMaxMs":2}}"#).unwrap();
        assert!(matches!(update_config_impl(&conn, &bad), Err(AppError::InputInvalid(_))));
        assert_eq!(get_config_impl(&conn).unwrap().agent_retry.max_retries, 3); // 仍是旧值
    }

    #[test]
    fn test_key_prefers_draft_and_drops_blank() {
        // 草稿非空 → 直接用草稿，绝不触碰 keyring（用一个几乎不可能有钥的 provider 名保证确定性）。
        assert_eq!(
            resolve_test_key(Some("sk-draft".into()), "__no_such_provider__").as_deref(),
            Some("sk-draft")
        );
        // 草稿仅空白 → 视同无草稿，回落 keyring∨env（该 provider 无条目 → None）。
        assert_eq!(
            resolve_test_key(Some("   ".into()), "__no_such_provider__"),
            None
        );
    }
}
