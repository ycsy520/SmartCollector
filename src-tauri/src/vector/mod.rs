//! 向量检索层（01 §vector）。
//! `VectorStore` trait 定义 upsert/search/get/delete；纯余弦 + top-k 排序逻辑无依赖，
//! 由 `MemoryStore`（离线/测试实现）驱动验收「写入→检索 top-k 有序」。
//! 持久化后端 `sqlite_vec`（vec0 虚表）已落地；语义路对 command 层的出口在 `semantic`。

pub mod embedding;
pub mod semantic;
pub mod sqlite_vec;

use std::collections::HashMap;

use crate::error::AppError;

/// 一条检索命中。
#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub fragment_id: String,
    pub score: f32,
}

/// 向量存储接口。实现方可以是内存（离线/测试）或 sqlite-vec（持久化）。
pub trait VectorStore {
    /// 写入/覆盖某片段的向量。`model` 变更应触发全量重建（见 03 §3.4）。
    fn upsert(
        &mut self,
        fragment_id: &str,
        result_version: i64,
        model: &str,
        embedding: &[f32],
    ) -> Result<(), AppError>;
    /// 用查询向量检索最相似的 top_k（余弦降序）。
    fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<VectorHit>, AppError>;
    /// 取某片段已存的向量（无则 None）。用于"以条目查近邻"（02 §7.4 `list_related`），
    /// 免得为一次相关推荐再花一次 embedding 调用。
    fn get(&self, fragment_id: &str) -> Result<Option<Vec<f32>>, AppError>;
    fn delete(&mut self, fragment_id: &str) -> Result<(), AppError>;
}

/// 余弦相似度；零范数或维度不符返回 0（不 panic）。
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// 内存向量库：离线核心与单测用；同时是 brute 暴力余弦的算法参考实现。
/// 持久化后端（sqlite-vec）落地前，不写库、进程退出即失。
#[derive(Default)]
pub struct MemoryStore {
    model: Option<String>,
    dim: usize,
    // fragment_id -> (version, embedding)
    entries: HashMap<String, (i64, Vec<f32>)>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl VectorStore for MemoryStore {
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
        // 换模型 → 全量重建（清空既有向量），对齐 03 §3.4「换 embedding 模型 = 删表重建」。
        if let Some(m) = &self.model {
            if m != model {
                self.entries.clear();
            }
        }
        // 维度与既有集合不一致：拒写（防污染检索）。
        if !self.entries.is_empty() && self.dim != embedding.len() {
            return Err(AppError::VectorUnavailable(format!(
                "维度不符：期望 {}，实际 {}",
                self.dim,
                embedding.len()
            )));
        }
        self.model = Some(model.to_string());
        self.dim = embedding.len();
        self.entries
            .insert(fragment_id.to_string(), (result_version, embedding.to_vec()));
        Ok(())
    }

    fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<VectorHit>, AppError> {
        let mut hits: Vec<VectorHit> = self
            .entries
            .iter()
            .map(|(id, (_v, vec))| VectorHit { fragment_id: id.clone(), score: cosine(query, vec) })
            .collect();
        // 降序；分数相等时按 id 稳定排序，保证可复现。
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.fragment_id.cmp(&b.fragment_id))
        });
        hits.truncate(top_k);
        Ok(hits)
    }

    fn delete(&mut self, fragment_id: &str) -> Result<(), AppError> {
        self.entries.remove(fragment_id);
        Ok(())
    }

    fn get(&self, fragment_id: &str) -> Result<Option<Vec<f32>>, AppError> {
        Ok(self.entries.get(fragment_id).map(|(_v, e)| e.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[f32]) -> Vec<f32> {
        xs.to_vec()
    }

    #[test]
    fn cosine_basics_and_guards() {
        assert!((cosine(&v(&[1.0, 0.0]), &v(&[1.0, 0.0])) - 1.0).abs() < 1e-6);
        assert!(cosine(&v(&[1.0, 0.0]), &v(&[0.0, 1.0])).abs() < 1e-6);
        assert!((cosine(&v(&[1.0, 1.0]), &v(&[2.0, 2.0])) - 1.0).abs() < 1e-6); // 方向同、模不同
        assert_eq!(cosine(&v(&[1.0, 2.0]), &v(&[1.0])), 0.0); // 维度不符
        assert_eq!(cosine(&v(&[0.0, 0.0]), &v(&[1.0, 1.0])), 0.0); // 零范数
    }

    #[test]
    fn search_returns_topk_ordered_by_similarity() {
        let mut s = MemoryStore::new();
        s.upsert("near", 1, "m", &v(&[1.0, 0.0, 0.0])).unwrap();
        s.upsert("mid", 1, "m", &v(&[0.7, 0.7, 0.0])).unwrap();
        s.upsert("far", 1, "m", &v(&[0.0, 0.0, 1.0])).unwrap();
        let hits = s.search(&v(&[1.0, 0.0, 0.0]), 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].fragment_id, "near");
        assert_eq!(hits[1].fragment_id, "mid");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn model_change_triggers_full_rebuild() {
        let mut s = MemoryStore::new();
        s.upsert("a", 1, "model-v1", &v(&[1.0, 0.0])).unwrap();
        s.upsert("b", 1, "model-v1", &v(&[0.0, 1.0])).unwrap();
        assert_eq!(s.entries.len(), 2);
        // 换模型 → 旧向量全清，只留新写入
        s.upsert("c", 1, "model-v2", &v(&[1.0, 1.0])).unwrap();
        assert_eq!(s.entries.len(), 1);
        assert!(s.entries.contains_key("c"));
    }

    #[test]
    fn dim_mismatch_is_rejected() {
        let mut s = MemoryStore::new();
        s.upsert("a", 1, "m", &v(&[1.0, 0.0, 0.0])).unwrap();
        assert!(s.upsert("b", 1, "m", &v(&[1.0, 0.0])).is_err());
        assert!(matches!(s.upsert("b", 1, "m", &v(&[])), Err(AppError::VectorUnavailable(_))));
    }

    #[test]
    fn get_returns_stored_vector_or_none() {
        let mut s = MemoryStore::new();
        s.upsert("a", 1, "m", &v(&[1.0, 2.0])).unwrap();
        assert_eq!(s.get("a").unwrap(), Some(v(&[1.0, 2.0])));
        assert_eq!(s.get("b").unwrap(), None);
    }

    #[test]
    fn delete_removes_entry() {
        let mut s = MemoryStore::new();
        s.upsert("a", 1, "m", &v(&[1.0, 0.0])).unwrap();
        s.delete("a").unwrap();
        assert!(s.search(&v(&[1.0, 0.0]), 5).unwrap().is_empty());
    }
}
