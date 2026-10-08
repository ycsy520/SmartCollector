//! config 表：KV 读写与默认值（03 文档 §3.5/§4；AppConfig 结构见 src/config.rs）。
//! API Key 不在此表（存 keyring，P7 接线），此处仅管配置 JSON。

use rusqlite::{params, Connection};

use super::now_iso;
use crate::config::AppConfig;
use crate::error::AppError;

/// 已退役的配置键（值已无意义，留着会让 config 表说谎）。**永不复用这些键名**：
/// 老库里用户可能存过 `enabled:true`，一旦被复用就会静默恢复一个已被否决的后台功能。
const RETIRED_KEYS: &[&str] = &["clip.watch"];

/// 键名 → JSON 取值。落库值为 JSON 文本（03 §3.5：字符串也带引号）。
fn entries(cfg: &AppConfig) -> Vec<(&'static str, String)> {
    vec![
        ("llm.primary", serde_json::to_string(&cfg.llm_primary).unwrap()),
        ("llm.fallback", serde_json::to_string(&cfg.llm_fallback).unwrap()),
        ("llm.embedding", serde_json::to_string(&cfg.llm_embedding).unwrap()),
        ("agent.retry", serde_json::to_string(&cfg.agent_retry).unwrap()),
        ("agent.auto_retry", serde_json::to_string(&cfg.agent_auto_retry).unwrap()),
        ("vector.backend", serde_json::to_string(&cfg.vector_backend).unwrap()),
        ("app.autostart", serde_json::to_string(&cfg.autostart).unwrap()),
        ("app.paste_shortcut", serde_json::to_string(&cfg.paste_shortcut).unwrap()),
        ("image.keep_original", serde_json::to_string(&cfg.keep_original_image).unwrap()),
        ("media.dir", serde_json::to_string(&cfg.media_dir).unwrap()),
        ("app.llm_enabled", serde_json::to_string(&cfg.llm_enabled).unwrap()),
    ]
}

fn load_map(conn: &Connection) -> Result<std::collections::HashMap<String, String>, AppError> {
    let mut stmt = conn
        .prepare("SELECT key, value FROM config")
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| AppError::DbRead(e.to_string()))
}

/// 取键；缺键或 JSON 损坏时回落默认值（老库升级/手工改库不至于起不来）。
fn get_or<T: serde::de::DeserializeOwned>(map: &std::collections::HashMap<String, String>, key: &str, fallback: T) -> T {
    map.get(key)
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or(fallback)
}

/// 首次建库写入默认配置；已有键不覆盖（INSERT OR IGNORE）。顺带清理已退役的键。
pub fn ensure_defaults(conn: &Connection, defaults: &AppConfig) -> Result<(), AppError> {
    let now = now_iso();
    for k in RETIRED_KEYS {
        conn.execute("DELETE FROM config WHERE key = ?1", params![k])?;
    }
    for (k, v) in entries(defaults) {
        conn.execute(
            "INSERT OR IGNORE INTO config (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![k, v, &now],
        )?;
    }
    // secrets.api_key_ref 只是 keyring 引用标记（03 §4），密钥本身永不入库
    conn.execute(
        "INSERT OR IGNORE INTO config (key, value, updated_at)
         VALUES ('secrets.api_key_ref', '\"keyring://com.smartcollector.desktop/llm_primary\"', ?1)",
        params![&now],
    )?;
    Ok(())
}

pub fn load(conn: &Connection) -> Result<AppConfig, AppError> {
    let map = load_map(conn)?;
    let d = AppConfig::default();
    Ok(AppConfig {
        llm_primary: get_or(&map, "llm.primary", d.llm_primary),
        llm_fallback: get_or(&map, "llm.fallback", d.llm_fallback),
        llm_embedding: get_or(&map, "llm.embedding", d.llm_embedding),
        agent_retry: get_or(&map, "agent.retry", d.agent_retry),
        agent_auto_retry: get_or(&map, "agent.auto_retry", d.agent_auto_retry),
        vector_backend: get_or(&map, "vector.backend", d.vector_backend),
        autostart: get_or(&map, "app.autostart", d.autostart),
        paste_shortcut: get_or(&map, "app.paste_shortcut", d.paste_shortcut),
        keep_original_image: get_or(&map, "image.keep_original", d.keep_original_image),
        media_dir: get_or(&map, "media.dir", d.media_dir),
        llm_enabled: get_or(&map, "app.llm_enabled", d.llm_enabled),
    })
}

/// 全量覆盖写。字段校验属 Command 层边界（update_config 先 validate 再落库）；
/// 默认配置本身是"未配置"态（model 为空），在 db 层校验会误伤。
pub fn save(conn: &Connection, cfg: &AppConfig) -> Result<(), AppError> {
    let now = now_iso();
    let tx = conn.unchecked_transaction()?;
    for (k, v) in entries(cfg) {
        tx.execute(
            "INSERT INTO config (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![k, v, &now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmEndpoint;

    #[test]
    fn defaults_ensure_and_load_roundtrip() {
        let conn = crate::db::test_conn();
        ensure_defaults(&conn, &AppConfig::default()).unwrap();
        let cfg = load(&conn).unwrap();
        assert_eq!(cfg, AppConfig::default());
        // 幂等：不覆盖已有值
        let mut changed = AppConfig::default();
        changed.vector_backend = "brute".into();
        save(&conn, &changed).unwrap();
        ensure_defaults(&conn, &AppConfig::default()).unwrap();
        assert_eq!(load(&conn).unwrap().vector_backend, "brute");
    }

    // 加速键与图片压缩开关都是后加的标量键：老库没有这一行也必须起得来（缺键回落默认，零迁移）。
    #[test]
    fn missing_late_added_scalar_rows_fall_back_to_default() {
        let conn = crate::db::test_conn();
        ensure_defaults(&conn, &AppConfig::default()).unwrap();
        conn.execute("DELETE FROM config WHERE key IN ('app.paste_shortcut', 'image.keep_original')", [])
            .unwrap();
        let cfg = load(&conn).unwrap();
        assert_eq!(cfg.paste_shortcut, crate::config::DEFAULT_PASTE_SHORTCUT);
        assert!(!cfg.keep_original_image, "缺行应回落 false");
        // ensure_defaults 再把缺的键补上（老库升级即自愈，无需迁移脚本）
        ensure_defaults(&conn, &AppConfig::default()).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM config WHERE key = 'image.keep_original'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    // 总开关是后加的标量键：老库缺行必须回落 true（打开=维持现状），且关闭能落库、重开能恢复。
    #[test]
    fn llm_enabled_roundtrip_and_default_true_on_missing_row() {
        let conn = crate::db::test_conn();
        ensure_defaults(&conn, &AppConfig::default()).unwrap();
        assert!(load(&conn).unwrap().llm_enabled, "默认应为开");
        // 缺行回落 true（同 keep_original_image 的自愈口径）
        conn.execute("DELETE FROM config WHERE key = 'app.llm_enabled'", []).unwrap();
        assert!(load(&conn).unwrap().llm_enabled, "缺行应回落 true");
        // 关闭 → 落库 → 读回 false；其余端点配置不受影响（非破坏）
        let mut off = AppConfig::default();
        off.llm_enabled = false;
        off.llm_primary.model = "keep-me".into();
        save(&conn, &off).unwrap();
        let reloaded = load(&conn).unwrap();
        assert!(!reloaded.llm_enabled);
        assert_eq!(reloaded.llm_primary.model, "keep-me", "关开关绝不清空端点");
    }

    #[test]
    fn save_validates_and_corrupt_value_falls_back() {
        let conn = crate::db::test_conn();
        let mut cfg = AppConfig::default();
        cfg.agent_retry.max_retries = 1;
        cfg.llm_fallback = Some(LlmEndpoint {
            provider: "openai_compat".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3".into(),
            timeout_s: 30,
            max_tokens: 1024,
        });
        save(&conn, &cfg).unwrap();
        assert_eq!(load(&conn).unwrap(), cfg);
        // 手工写坏 JSON → load 回落默认，不 panic
        conn.execute("UPDATE config SET value = '{broken' WHERE key = 'agent.retry'", [])
            .unwrap();
        assert_eq!(load(&conn).unwrap().agent_retry, AppConfig::default().agent_retry);
    }

    // 剪贴板被动监听已退役（2026-09-26 用户裁定）：老库残留的 clip.watch 行必须被清掉，
    // 否则 config 表里留着一个无人读的键，看起来像还能开某个后台功能。
    #[test]
    fn ensure_defaults_prunes_retired_keys() {
        let conn = crate::db::test_conn();
        ensure_defaults(&conn, &AppConfig::default()).unwrap();
        conn.execute(
            "INSERT INTO config (key, value, updated_at) VALUES ('clip.watch', '{\"enabled\":true}', 'x')",
            [],
        )
        .unwrap();
        ensure_defaults(&conn, &AppConfig::default()).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM config WHERE key = 'clip.watch'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}
