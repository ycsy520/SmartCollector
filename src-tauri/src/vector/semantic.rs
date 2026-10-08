//! 语义路装配（01 §vector）：把「embedding 端点是否可用」的判定、向量库构建、以及一次
//! 文本 → 向量 的阻塞调用收在一处，供 `commands::{search,curation}` 消费。
//!
//! 分层沿用 worker 的边界：纯逻辑（取已存向量查近邻）离线可测；真 HTTP 只在独立 std 线程内
//! 发生（reqwest blocking 会建自己的 tokio 运行时，跑在 tauri 线程上会 nested-runtime panic）。
//! 任何语义路失败都不该让用户看到报错——检索与相关推荐本来就是「有则更好」的增量。

use rusqlite::Connection;

use crate::config::{secrets, AppConfig, EmbeddingEndpoint};
use crate::db::Pool;
use crate::error::AppError;

use super::embedding::{Embedder, HttpEmbeddingTransport, OpenAiEmbedder};
use super::sqlite_vec::SqliteVecStore;
use super::{VectorHit, VectorStore};

/// embedding 端点是否配置齐全（总开关打开 + 有模型 + 有维度 + 取得到密钥）。
/// worker 落向量与 command 层语义路共用同一个闸门，避免两处判定漂移。
/// `llm_enabled` 关掉时语义/混合检索退化为纯关键词（`degraded`），与"没配 embedding"同一条路径。
pub fn embedding_ready(cfg: &AppConfig) -> bool {
    let emb = &cfg.llm_embedding;
    cfg.llm_enabled && !emb.model.trim().is_empty() && emb.dim > 0 && secrets::api_key_for(&emb.provider).is_some()
}

/// 只为删向量而建的句柄：有 embedding 模型与维度即可（删除不联网，**不要求 API key**）。
/// 缺配置=库里本就没有向量，调用方据此跳过（02 §2.8）。
pub fn purge_handle(pool: &Pool, cfg: &AppConfig) -> Option<SqliteVecStore> {
    let emb = &cfg.llm_embedding;
    if emb.model.trim().is_empty() || emb.dim <= 0 {
        return None;
    }
    SqliteVecStore::new(pool.clone(), &emb.model, emb.dim as usize).ok()
}

/// 语义索引：向量库 + 其 embedding 端点配置。缺配时不构建（`open` 返回 None）。
pub struct SemanticIndex {
    store: SqliteVecStore,
    endpoint: EmbeddingEndpoint,
}

impl SemanticIndex {
    /// 配置齐全且虚表可建时返回实例；否则 None（调用方据此降级）。
    pub fn open(pool: &Pool, cfg: &AppConfig) -> Option<Self> {
        if !embedding_ready(cfg) {
            return None;
        }
        let emb = &cfg.llm_embedding;
        let store = SqliteVecStore::new(pool.clone(), &emb.model, emb.dim as usize).ok()?;
        Some(Self { store, endpoint: emb.clone() })
    }

    /// 只读取已存向量的近邻路（不联网），供相关推荐与测试注入。
    pub fn store(&self) -> &dyn VectorStore {
        &self.store
    }

    /// 查询文本 → 一次 embedding → 向量近邻。网络调用在独立 std 线程内完成。
    pub fn near_text(&self, text: &str, k: usize) -> Result<Vec<VectorHit>, AppError> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let (endpoint, key, text) = (
            self.endpoint.clone(),
            secrets::api_key_for(&self.endpoint.provider).unwrap_or_default(),
            text.to_string(),
        );
        let handle = std::thread::spawn(move || {
            let transport = HttpEmbeddingTransport::new();
            OpenAiEmbedder::new(endpoint, key, &transport).embed(&text)
        });
        let query = match handle.join() {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(AppError::VectorUnavailable("embedding 线程异常退出".into())),
        };
        self.store.search(&query, k)
    }
}

/// 以条目查近邻（02 §7.4）：直接用库里已存的向量，省一次 embedding 调用。
/// KNN 会把自身算作最近邻（距离 0），故多取一名再剔除；未入向量库的条目返回空。
pub fn near_fragment(store: &dyn VectorStore, fragment_id: &str, k: usize) -> Result<Vec<VectorHit>, AppError> {
    if k == 0 {
        return Ok(Vec::new());
    }
    let Some(vec) = store.get(fragment_id)? else {
        return Ok(Vec::new());
    };
    let mut hits = store.search(&vec, k + 1)?;
    hits.retain(|h| h.fragment_id != fragment_id);
    hits.truncate(k);
    Ok(hits)
}

/// 语义候选走一遍与关键词路相同的可见性规则（墓碑不可见），顺序按相似度不变。
pub fn live_hits(conn: &Connection, hits: &[VectorHit], exclude_layers: &[&str]) -> Result<Vec<VectorHit>, AppError> {
    let ids: Vec<String> = hits.iter().map(|h| h.fragment_id.clone()).collect();
    let live = crate::db::fragments::live_id_set(conn, &ids, exclude_layers)?;
    Ok(hits.iter().filter(|h| live.contains(&h.fragment_id)).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::MemoryStore;

    fn cfg_with(model: &str, dim: u32) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.llm_embedding = EmbeddingEndpoint {
            provider: "sc_semantic_test".into(),
            base_url: "https://api.example.com/v1".into(),
            model: model.into(),
            dim,
        };
        cfg
    }

    #[test]
    fn embedding_ready_gate() {
        // 缺模型 / 缺维度：闸门在查密钥前短路，不触碰 keyring 与环境变量。
        assert!(!embedding_ready(&cfg_with("  ", 1024)));
        assert!(!embedding_ready(&cfg_with("m", 0)));
        // 模型与维度齐备 → 只看取得到密钥与否（用独立 provider 名注入，不与真实密钥冲突）。
        std::env::set_var("SC_SEMANTIC_TEST_API_KEY", "sk-placeholder-not-real");
        assert!(embedding_ready(&cfg_with("text-embedding-v3", 1024)));
        // 总开关关闭：即便模型/维度/密钥齐全也必须判不就绪（真被读，非惰性摆设）→ 检索降级关键词。
        let mut off = cfg_with("text-embedding-v3", 1024);
        off.llm_enabled = false;
        assert!(!embedding_ready(&off), "llm_enabled=false 应压过一切就绪条件");
        std::env::remove_var("SC_SEMANTIC_TEST_API_KEY");
    }

    #[test]
    fn near_fragment_uses_stored_vector_and_drops_self() {
        let mut s = MemoryStore::new();
        s.upsert("q", 1, "m", &[1.0, 0.0, 0.0]).unwrap();
        s.upsert("a", 1, "m", &[0.9, 0.1, 0.0]).unwrap();
        s.upsert("b", 1, "m", &[0.0, 1.0, 0.0]).unwrap();
        let hits = near_fragment(&s, "q", 2).unwrap();
        assert_eq!(hits.iter().map(|h| h.fragment_id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        // 未入向量库的条目：无近邻可说
        assert!(near_fragment(&s, "absent", 5).unwrap().is_empty());
        assert!(near_fragment(&s, "q", 0).unwrap().is_empty());
    }

    /// 真链路验收：open 闸门 → 独立线程 embedding → KNN。需要 .env 里的 QWEN_API_KEY。
    /// 顺带验语义排序确实按含义、不按字面（查询词与"红烧肉"零重合）。
    /// `cargo test --lib -- --ignored semantic_index_real_qwen`
    #[test]
    #[ignore = "需要 QWEN_API_KEY 真实密钥与网络"]
    fn semantic_index_real_qwen() {
        use crate::config::AppConfig;
        secrets::load_dotenv();
        let dir = std::env::temp_dir().join(format!("sc-sem-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = crate::db::init_pool(&dir.join("smart.db")).unwrap();
        let cfg = AppConfig::qwen();
        let ep = &cfg.llm_embedding;
        let key = secrets::api_key_for(&ep.provider).expect("请先在 .env 设置 QWEN_API_KEY");
        let texts = [
            ("db", "今天把服务器数据库做了备份，一切顺利"),
            ("food", "红烧肉的做法是先焯水再慢炖收汁"),
        ];
        {
            let conn = pool.get().unwrap();
            for (id, txt) in &texts {
                conn.execute(
                    "INSERT INTO fragments(id,content,source,content_hash,char_count,created_at,updated_at)
                     VALUES(?1,?2,'manual',?3,?4,'2026-01-01','2026-01-01')",
                    rusqlite::params![id, txt, format!("h-{id}"), txt.chars().count() as i64],
                )
                .unwrap();
            }
        }
        // 先按 worker 的姿势把向量灌进库
        let transport = HttpEmbeddingTransport::new();
        let embedder = OpenAiEmbedder::new(ep.clone(), &key, &transport);
        let mut store = SqliteVecStore::new(pool.clone(), &ep.model, ep.dim as usize).unwrap();
        for (id, txt) in &texts {
            let v = embedder.embed(txt).expect("真 embedding 调用失败");
            assert_eq!(v.len(), ep.dim as usize, "返回维度应与配置一致(1024)");
            store.upsert(id, 1, &ep.model, &v).unwrap();
        }
        // 再走 command 层用的那条路：open 闸门 + 独立线程 embedding + KNN
        let idx = SemanticIndex::open(&pool, &cfg).expect("qwen 预设 + 密钥应通过闸门");
        let hits = idx.near_text("数据库备份与检索", 2).expect("语义路查询失败");
        assert_eq!(hits.len(), 2, "库里两条向量都应被 KNN 返回");
        assert_eq!(hits[0].fragment_id, "db", "语义最近邻应是数据库那条，而非字面无关的红烧肉");
        assert!(hits[0].score > hits[1].score);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
