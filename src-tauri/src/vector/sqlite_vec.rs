//! sqlite-vec 持久化后端（03 §3.4）。
//! 映射主表 `vector_refs`（生命周期/版本/模型/dim）由 schema.sql 建；本模块负责
//! 向量虚表 `vec_embeddings`（`fragment_id TEXT PRIMARY KEY, embedding float[dim]`，
//! cosine 距离）的建表、写入、KNN 检索与删除，并落实「换 embedding 模型 = 删表重建」。
//! 虚表 dim 由 config 决定，故在建库/换模型时由本模块创建，不入迁移（见 schema.sql 注）。
//!
//! 依赖 `sqlite-vec`，经 `sqlite3_auto_extension` 注册进 rusqlite bundled SQLite；
//! 注册须早于任何连接打开（见 `db::init_pool` 与 `ensure_extension_registered`）。

use std::sync::Once;

use rusqlite::Connection;

use crate::db::Pool;
use crate::error::AppError;

use super::VectorStore;

static REGISTER: Once = Once::new();

/// 把 sqlite-vec 注册为 SQLite 自动扩展（进程内仅一次）。
/// 之后新建的每个连接都会挂上 vec0 模块。须在建池/开连接前调用。
pub fn ensure_extension_registered() {
    REGISTER.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// f32 切片 → little-endian 字节（sqlite-vec 的 float[N] blob 序列化约定）。
fn f32_slice_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for &f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// sqlite-vec 虚表 + vector_refs 主表组成的持久向量库。
pub struct SqliteVecStore {
    pool: Pool,
    model: String,
    dim: usize,
}

/// little-endian 字节 → f32 向量（`get` 用）。空或长度非 4 倍数视为脏数据 → None。
fn blob_to_f32_slice(b: &[u8]) -> Option<Vec<f32>> {
    if b.is_empty() || b.len() % 4 != 0 {
        return None;
    }
    Some(
        b.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

impl SqliteVecStore {
    /// 以当前 embedding 模型名与维度建库：确保虚表存在；若既有向量的模型不同则全量重建。
    pub fn new(pool: Pool, model: &str, dim: usize) -> Result<Self, AppError> {
        ensure_extension_registered();
        if dim == 0 {
            return Err(AppError::VectorUnavailable("dim 不能为 0".into()));
        }
        let conn = pool.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        if model_changed(&conn, model)? {
            rebuild(&conn)?;
        }
        ensure_vec_table(&conn, dim)?;
        Ok(Self { pool, model: model.to_string(), dim })
    }
}

/// 既有 vector_refs 是否存在与本 store 不同模型的行（触发全量重建）。
fn model_changed(conn: &Connection, model: &str) -> Result<bool, AppError> {
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM vector_refs WHERE model != ?1",
            [model],
            |r| r.get(0),
        )
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    Ok(n > 0)
}

/// 删虚表 + 清空映射主表（换模型/换维度全量重建的前半）。
fn rebuild(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch("DROP TABLE IF EXISTS vec_embeddings; DELETE FROM vector_refs;")
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
    Ok(())
}

/// 按当前 dim 建 vec0 虚表（cosine 距离，distance∈[0,2]，score=1-distance）。
fn ensure_vec_table(conn: &Connection, dim: usize) -> Result<(), AppError> {
    let sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_embeddings USING vec0(
            fragment_id TEXT PRIMARY KEY,
            embedding float[{dim}] distance_metric=cosine
        );"
    );
    conn.execute_batch(&sql)
        .map_err(|e| AppError::DbWrite(format!("建 vec0 虚表失败: {e}")))?;
    Ok(())
}

impl VectorStore for SqliteVecStore {
    fn upsert(
        &mut self,
        fragment_id: &str,
        result_version: i64,
        model: &str,
        embedding: &[f32],
    ) -> Result<(), AppError> {
        if embedding.is_empty() {
            return Err(AppError::VectorUnavailable("空向量".into()));
        }
        if embedding.len() != self.dim {
            return Err(AppError::VectorUnavailable(format!(
                "维度不符：期望 {}，实际 {}",
                self.dim,
                embedding.len()
            )));
        }
        // 运行期写入不同模型视为需要重建，拒写以防混模型残留向量。
        if model != self.model {
            return Err(AppError::VectorUnavailable(format!(
                "模型不一致：store={}，写入={model}（换模型请重建库）",
                self.model
            )));
        }
        let conn = self.pool.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        let blob = f32_slice_to_blob(embedding);
        let now = crate::db::now_iso();
        // vec0 无原地更新：先删同主键旧向量再插；与 vector_refs 同事务保一致。
        conn.execute("DELETE FROM vec_embeddings WHERE fragment_id = ?1", [fragment_id])
            .map_err(|e| AppError::DbWrite(e.to_string()))?;
        conn.execute(
            "INSERT INTO vec_embeddings(fragment_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![fragment_id, blob],
        )
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
        conn.execute(
            "INSERT INTO vector_refs(fragment_id, result_version, model, dim, embedded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(fragment_id) DO UPDATE SET
               result_version = excluded.result_version,
               model = excluded.model,
               dim = excluded.dim,
               embedded_at = excluded.embedded_at",
            rusqlite::params![fragment_id, result_version, model, self.dim as i64, now],
        )
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
        Ok(())
    }

    fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<super::VectorHit>, AppError> {
        if query.len() != self.dim {
            return Err(AppError::VectorUnavailable(format!(
                "查询维度不符：期望 {}，实际 {}",
                self.dim,
                query.len()
            )));
        }
        if top_k == 0 {
            return Ok(Vec::new());
        }
        let conn = self.pool.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        let blob = f32_slice_to_blob(query);
        let mut stmt = conn
            .prepare(
                "SELECT fragment_id, distance FROM vec_embeddings
                 WHERE embedding MATCH ?1 AND k = ?2
                 ORDER BY distance",
            )
            .map_err(|e| AppError::DbRead(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![blob, top_k as i64], |r| {
                let id: String = r.get(0)?;
                let dist: f64 = r.get(1)?;
                // cosine 距离 ∈ [0,2]，转相似度分数（越大越相似），与 MemoryStore 语义一致。
                Ok(super::VectorHit { fragment_id: id, score: (1.0 - dist) as f32 })
            })
            .map_err(|e| AppError::DbRead(e.to_string()))?;
        let mut hits = Vec::new();
        for row in rows {
            hits.push(row.map_err(|e| AppError::DbRead(e.to_string()))?);
        }
        Ok(hits)
    }

    fn get(&self, fragment_id: &str) -> Result<Option<Vec<f32>>, AppError> {
        let conn = self.pool.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding FROM vec_embeddings WHERE fragment_id = ?1",
                [fragment_id],
                |r| r.get(0),
            )
            .ok();
        Ok(blob.as_deref().and_then(blob_to_f32_slice))
    }

    fn delete(&mut self, fragment_id: &str) -> Result<(), AppError> {
        let conn = self.pool.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        conn.execute("DELETE FROM vec_embeddings WHERE fragment_id = ?1", [fragment_id])
            .map_err(|e| AppError::DbWrite(e.to_string()))?;
        conn.execute("DELETE FROM vector_refs WHERE fragment_id = ?1", [fragment_id])
            .map_err(|e| AppError::DbWrite(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个已迁移的文件库连接池（vec0 与 vector_refs 均在），并保证扩展已注册。
    fn pool_with_schema() -> (Pool, std::path::PathBuf) {
        ensure_extension_registered();
        let dir = std::env::temp_dir().join(format!("sc-vec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("smart.db");
        let pool = crate::db::init_pool(&path).unwrap();
        (pool, dir)
    }

    fn insert_fragment(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO fragments(id,content,source,content_hash,char_count,created_at,updated_at)
             VALUES(?1,'x','manual',?2,1,'2026-01-01','2026-01-01')",
            rusqlite::params![id, format!("h-{id}")],
        )
        .unwrap();
    }

    #[test]
    fn sqlite_vec_upsert_search_knn_orders_by_similarity() {
        let (pool, dir) = pool_with_schema();
        {
            let conn = pool.get().unwrap();
            insert_fragment(&conn, "near");
            insert_fragment(&conn, "mid");
            insert_fragment(&conn, "far");
        }
        let dim = 3usize;
        let mut s = SqliteVecStore::new(pool.clone(), "test-model", dim).unwrap();
        s.upsert("near", 1, "test-model", &[1.0, 0.0, 0.0]).unwrap();
        s.upsert("mid", 1, "test-model", &[0.7, 0.7, 0.0]).unwrap();
        s.upsert("far", 1, "test-model", &[0.0, 0.0, 1.0]).unwrap();

        let hits = s.search(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].fragment_id, "near");
        assert_eq!(hits[1].fragment_id, "mid");
        assert!(hits[0].score > hits[1].score);
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // `get` 是"以条目查近邻"（list_related）的前提：虚表要能按主键读回原向量。
    #[test]
    fn get_roundtrips_vector_and_misses_absent() {
        let (pool, dir) = pool_with_schema();
        {
            let conn = pool.get().unwrap();
            insert_fragment(&conn, "a");
        }
        let mut s = SqliteVecStore::new(pool.clone(), "m", 3).unwrap();
        s.upsert("a", 1, "m", &[0.25, -0.5, 1.0]).unwrap();
        let got = s.get("a").unwrap().expect("应读回向量");
        assert_eq!(got.len(), 3);
        // 精确回读（le 字节往返无损），含负数
        assert_eq!(got, vec![0.25f32, -0.5, 1.0]);
        assert!(s.get("missing").unwrap().is_none());
        s.delete("a").unwrap();
        assert!(s.get("a").unwrap().is_none());
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_overwrites_and_delete_removes() {
        let (pool, dir) = pool_with_schema();
        {
            let conn = pool.get().unwrap();
            insert_fragment(&conn, "a");
        }
        let mut s = SqliteVecStore::new(pool.clone(), "m", 2).unwrap();
        s.upsert("a", 1, "m", &[1.0, 0.0]).unwrap();
        // 覆盖：改为与查询正交，相似度应下降。
        s.upsert("a", 2, "m", &[0.0, 1.0]).unwrap();
        let hits = s.search(&[1.0, 0.0], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score.abs() < 1e-3, "覆盖后应近似正交，score≈0");
        s.delete("a").unwrap();
        assert!(s.search(&[1.0, 0.0], 5).unwrap().is_empty());
        let conn = pool.get().unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM vector_refs WHERE fragment_id='a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "delete 应同时清 vector_refs");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dim_and_model_guards_reject_write() {
        let (pool, dir) = pool_with_schema();
        {
            let conn = pool.get().unwrap();
            insert_fragment(&conn, "a");
            insert_fragment(&conn, "b");
        }
        let mut s = SqliteVecStore::new(pool.clone(), "m1", 3).unwrap();
        // 维度不符
        assert!(matches!(s.upsert("a", 1, "m1", &[1.0, 0.0]), Err(AppError::VectorUnavailable(_))));
        // 空向量
        assert!(matches!(s.upsert("a", 1, "m1", &[]), Err(AppError::VectorUnavailable(_))));
        // 运行期模型不一致
        assert!(matches!(
            s.upsert("a", 1, "m2", &[1.0, 0.0, 0.0]),
            Err(AppError::VectorUnavailable(_))
        ));
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_change_rebuilds_existing_vectors() {
        let (pool, dir) = pool_with_schema();
        {
            let conn = pool.get().unwrap();
            insert_fragment(&conn, "a");
            insert_fragment(&conn, "b");
            insert_fragment(&conn, "c");
        }
        {
            let mut s = SqliteVecStore::new(pool.clone(), "v1", 2).unwrap();
            s.upsert("a", 1, "v1", &[1.0, 0.0]).unwrap();
            s.upsert("b", 1, "v1", &[0.0, 1.0]).unwrap();
        }
        // 换模型建库 → 全量重建：旧向量清空，只留新写入。
        let mut s2 = SqliteVecStore::new(pool.clone(), "v2", 2).unwrap();
        {
            let conn = pool.get().unwrap();
            let n: i64 = conn
                .query_row("SELECT count(*) FROM vector_refs", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "换模型建库应清空既有 vector_refs");
        }
        s2.upsert("c", 1, "v2", &[1.0, 1.0]).unwrap(); // 重建后虚表可用
        assert_eq!(s2.search(&[1.0, 1.0], 5).unwrap().len(), 1);
        drop(s2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sqlite-vec 最小实验（保留）：纯虚表层验证 Windows/MSVC 能编过、vec0 能建、KNN 有序。
    #[test]
    fn experiment_vec0_knn_minimal() {
        use rusqlite::Connection;
        ensure_extension_registered();
        let dim = 1024usize;
        let db = Connection::open_in_memory().expect("open in-memory");
        db.execute_batch("CREATE VIRTUAL TABLE vec_embeddings USING vec0(embedding float[1024]);")
            .expect("create vec0");

        let mut basis0 = vec![0f32; dim];
        basis0[0] = 1.0;
        let mut basis1 = vec![0f32; dim];
        basis1[1] = 1.0;
        let mut mix = vec![0f32; dim];
        mix[0] = 0.9;
        mix[1] = 0.1;

        let mut id = 1i64;
        for v in [&basis0, &basis1, &mix] {
            db.execute(
                "INSERT INTO vec_embeddings(rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, f32_slice_to_blob(v)],
            )
            .expect("insert vector");
            id += 1;
        }

        let mut stmt = db
            .prepare(
                "SELECT rowid, distance FROM vec_embeddings WHERE embedding MATCH ?1 AND k = 2 ORDER BY distance",
            )
            .expect("prepare knn");
        let mut rows = stmt
            .query_map(rusqlite::params![f32_slice_to_blob(&basis0)], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })
            .expect("run knn");
        let mut hits = Vec::new();
        while let Some(row) = rows.next() {
            hits.push(row.expect("knn row"));
        }
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, 1, "最近邻应是 basis0(rowid=1)");
        assert_eq!(hits[1].0, 3, "次近邻应是 mix(rowid=3)");
        assert!(hits[0].1 <= hits[1].1);
    }

    /// 端到端真调（P6 验收·需 QWEN_API_KEY）：真 embedding → 落 sqlite-vec 虚表 → KNN 检索。
    /// 你填好 `.env` 的 `QWEN_API_KEY=` 后跑：
    /// `cargo test --lib -- --ignored real_embed_store_search_qwen`
    #[test]
    #[ignore = "需要 QWEN_API_KEY 真实密钥与网络"]
    fn real_embed_store_search_qwen() {
        use crate::config::secrets;
        use crate::vector::embedding::{Embedder, HttpEmbeddingTransport, OpenAiEmbedder};

        secrets::load_dotenv();
        let ep = crate::config::AppConfig::qwen().llm_embedding;
        let key = secrets::api_key_for(&ep.provider).expect("请先在 .env 设置 QWEN_API_KEY");
        let t = HttpEmbeddingTransport::new();
        let embedder = OpenAiEmbedder::new(ep.clone(), &key, &t);

        let texts = [
            "今天把服务器数据库做了备份，一切顺利",
            "红烧肉的做法是先焯水再慢炖收汁",
            "向量检索用余弦相似度衡量语义接近程度",
        ];
        let (pool, dir) = pool_with_schema();
        {
            let conn = pool.get().unwrap();
            for (i, _) in texts.iter().enumerate() {
                insert_fragment(&conn, &format!("f{i}"));
            }
        }
        let mut store = SqliteVecStore::new(pool.clone(), &ep.model, ep.dim as usize).unwrap();
        for (i, txt) in texts.iter().enumerate() {
            let vec = embedder.embed(txt).expect("真 embedding 调用失败");
            assert_eq!(vec.len(), ep.dim as usize, "返回维度应与配置一致(1024)");
            store.upsert(&format!("f{i}"), 1, &ep.model, &vec).unwrap();
        }
        let q = embedder.embed("数据库备份与检索").expect("query embedding 失败");
        let hits = store.search(&q, 2).unwrap();
        assert!(!hits.is_empty(), "KNN 应命中");
        println!("qwen e2e -> top={} 命中 {:?} (score={:.3})", hits.len(), hits[0].fragment_id, hits[0].score);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
