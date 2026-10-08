//! 搜索 command（02 §3）。keyword 路走 FTS5；semantic/hybrid 的向量路由 `vector::semantic` 提供，
//! 两路各取 top-N 后按 RRF 融合（02 §3.1）。语义路不可用（未配置 embedding / embedding 调用失败）
//! 时自动降级为纯 keyword 并置 `degraded: true`——降级不是错误，不向前端抛异常。
use std::collections::HashMap;

use rusqlite::Connection;
use tauri::State;

use crate::db::{config_store, fragments};
use crate::dto::search as sd;
use crate::error::AppError;
use crate::state::AppState;
use crate::vector::semantic::SemanticIndex;
use crate::vector::VectorHit;

use super::fragment::{build_detail, to_summary};

/// RRF 平滑常数：两路名次混排时的衰减基准（取论文常用的 60，越大越偏向头部一致性）。
const RRF_K: f64 = 60.0;

/// 检索口径（02 §3.1，用户 2026-09-26 裁定「检索收口到归档层」）：搜索是"查我已存的"，
/// 缓冲区是**待分拣的在制品**、垃圾站是**已丢弃的**，都不该出现在检索结果里；两处显式入口各自可达。
/// 两条路必须同一口径，否则 hybrid 会把未分拣条目混进结果，前端过滤只会让分数对不上名次。
const SEARCH_EXCLUDED: &[&str] = &["buffer", "trash"];

fn to_hit(conn: &Connection, id: &str, score: f64, matched: sd::Matched) -> Result<sd::SearchHit, AppError> {
    Ok(sd::SearchHit {
        fragment: to_summary(&build_detail(conn, id)?),
        score,
        matched,
    })
}

/// RRF 融合：`score = Σ 1/(RRF_K + rank)`，rank 从 1 起。只用名次、不吃两路各自的分数尺度——
/// bm25 与余弦不可直接相加，这正是契约点名为混合排序方案的原因。
/// 返回值归一化到最优命中（RRF 原量级 ~0.016，按百分比展示会变成"2%"这种无意义数字）。
fn rrf_merge(keyword: &[(String, f64)], semantic: &[VectorHit], limit: usize) -> Vec<(String, sd::Matched, f64)> {
    #[derive(Default)]
    struct Fuse {
        score: f64,
        keyword: bool,
        semantic: bool,
    }
    let mut merged: HashMap<String, Fuse> = HashMap::new();
    for (rank, (id, _)) in keyword.iter().enumerate() {
        let e = merged.entry(id.clone()).or_default();
        e.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        e.keyword = true;
    }
    for (rank, hit) in semantic.iter().enumerate() {
        let e = merged.entry(hit.fragment_id.clone()).or_default();
        e.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        e.semantic = true;
    }
    let mut rows: Vec<(String, sd::Matched, f64)> = merged
        .into_iter()
        .map(|(id, f)| {
            let matched = match (f.keyword, f.semantic) {
                (true, true) => sd::Matched::Both,
                (false, true) => sd::Matched::Semantic,
                _ => sd::Matched::Keyword,
            };
            (id, matched, f.score)
        })
        .collect();
    // 降序；同分按 id 稳定，保证同一库同一查询的结果可复现。
    rows.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    rows.truncate(limit);
    let top = rows.first().map(|r| r.2).unwrap_or(0.0);
    if top > 0.0 {
        for r in rows.iter_mut() {
            r.2 /= top;
        }
    }
    rows
}

/// `semantic = None` → 语义路不可用；`Some(&[])` → 语义路可用但库里还没有向量。
pub(crate) fn search_fragments_impl(
    conn: &Connection,
    q: &sd::SearchQuery,
    semantic: Option<&[VectorHit]>,
) -> Result<sd::SearchResult, AppError> {
    if q.text.trim().is_empty() {
        return Err(AppError::InputEmpty);
    }
    let limit = q.limit.unwrap_or(20).clamp(1, 50) as usize;
    let mode = q.mode.unwrap_or(sd::SearchMode::Hybrid);
    let want_semantic = !matches!(mode, sd::SearchMode::Keyword);
    // 向量只认 id，看不见 layer 与软删墓碑——与关键词路用同一可见性规则（SEARCH_EXCLUDED）。
    // 已知偏差：KNN 已在虚表侧按 top-N 截断，这里再裁剪只能让结果变短，不会漏进不该见的条目。
    let semantic: Option<Vec<VectorHit>> = if want_semantic {
        semantic.map(|h| crate::vector::semantic::live_hits(conn, h, SEARCH_EXCLUDED)).transpose()?
    } else {
        None
    };
    let degraded = want_semantic && semantic.is_none();

    let mut items: Vec<sd::SearchHit> = Vec::new();
    match (mode, semantic.as_deref()) {
        // 纯语义：没有语义命中就是没有，不拿关键词凑数（用户显式选了这一路）
        (sd::SearchMode::Semantic, Some(hits)) => {
            for hit in hits.iter().take(limit) {
                items.push(to_hit(conn, &hit.fragment_id, hit.score as f64, sd::Matched::Semantic)?);
            }
        }
        // 混排：两路名次融合
        (sd::SearchMode::Hybrid, Some(hits)) => {
            let keyword = fragments::fts_search(conn, &q.text, limit as i64, SEARCH_EXCLUDED)?;
            for (id, matched, score) in rrf_merge(&keyword, hits, limit) {
                items.push(to_hit(conn, &id, score, matched)?);
            }
        }
        // keyword 路，或语义路不可用时的降级路
        _ => {
            for (id, score) in fragments::fts_search(conn, &q.text, limit as i64, SEARCH_EXCLUDED)? {
                items.push(to_hit(conn, &id, score, sd::Matched::Keyword)?);
            }
        }
    }
    Ok(sd::SearchResult { items, degraded })
}

#[tauri::command]
pub fn search_fragments(state: State<'_, AppState>, query: sd::SearchQuery) -> Result<sd::SearchResult, AppError> {
    let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
    let semantic = if matches!(query.mode.unwrap_or(sd::SearchMode::Hybrid), sd::SearchMode::Keyword) {
        None
    } else {
        let cfg = config_store::load(&conn)?;
        let top = query.limit.unwrap_or(20).clamp(1, 50) as usize;
        // embedding 失败（缺配/超时/上游异常）→ None → 下面按 02 §3.1 降级，不外抛。
        SemanticIndex::open(&state.db, &cfg).and_then(|idx| idx.near_text(&query.text, top).ok())
    };
    search_fragments_impl(&conn, &query, semantic.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::fragment::submit_text_impl;
    use crate::dto::fragment::SubmitInput;

    fn query(text: &str, mode: Option<sd::SearchMode>) -> sd::SearchQuery {
        sd::SearchQuery { text: text.into(), mode, limit: None }
    }

    fn hit(id: &str, score: f32) -> VectorHit {
        VectorHit { fragment_id: id.into(), score }
    }

    /// 收集并**放行到归档层**——检索只在归档层（02 §3.1），所以能被搜到的条目必须先分拣。
    fn submit(conn: &Connection, content: &str) -> String {
        let id = submit_text_impl(
            conn,
            &SubmitInput { content: content.into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap()
        .fragment_id;
        fragments::set_fragment_layer(conn, &id, "archived").unwrap();
        id
    }

    /// 留在缓冲区（未分拣）的条目
    fn submit_buffer(conn: &Connection, content: &str) -> String {
        submit_text_impl(
            conn,
            &SubmitInput { content: content.into(), title: None, note: None, skill_ids: vec![] },
        )
        .unwrap()
        .fragment_id
    }

    #[test]
    fn search_is_scoped_to_archived_layer() {
        let conn = crate::db::test_conn();
        let in_buffer = submit_buffer(&conn, "未分拣的关键词检索条目");
        let in_trash = submit(&conn, "已丢弃的关键词检索条目");
        fragments::set_fragment_layer(&conn, &in_trash, "trash").unwrap();
        let archived = submit(&conn, "已归档的关键词检索条目");
        // 关键词路：只有归档层可见
        let kw = search_fragments_impl(&conn, &query("关键词检索", Some(sd::SearchMode::Keyword)), None).unwrap();
        assert_eq!(kw.items.iter().map(|h| h.fragment.id.as_str()).collect::<Vec<_>>(), vec![archived.as_str()]);
        // 语义路：同一口径（虚表看不见 layer，靠 live_hits 裁剪；命中里混入缓冲区/垃圾站也不得出现）
        let sem = vec![hit(&in_buffer, 0.9), hit(&in_trash, 0.8), hit(&archived, 0.7)];
        let se = search_fragments_impl(&conn, &query("关键词检索", Some(sd::SearchMode::Semantic)), Some(&sem)).unwrap();
        assert_eq!(se.items.iter().map(|h| h.fragment.id.as_str()).collect::<Vec<_>>(), vec![archived.as_str()]);
        // 混排同样不透出被排除层
        let hy = search_fragments_impl(&conn, &query("关键词检索", None), Some(&sem)).unwrap();
        assert!(hy.items.iter().all(|h| h.fragment.id == archived));
    }

    #[test]
    fn keyword_hit_and_hybrid_degrade() {
        let conn = crate::db::test_conn();
        submit(&conn, "SQLite FTS5 分词检索");
        let kw = search_fragments_impl(&conn, &query("分词检索", Some(sd::SearchMode::Keyword)), None).unwrap();
        assert_eq!(kw.items.len(), 1);
        assert!(!kw.degraded); // 纯 keyword 不算降级
        assert_eq!(kw.items[0].matched, sd::Matched::Keyword);
        let hy = search_fragments_impl(&conn, &query("分词检索", None), None).unwrap();
        assert!(hy.degraded); // hybrid 语义路不可用 → 降级为 keyword
        assert_eq!(hy.items.len(), 1);
        // 显式 semantic 且语义路不可用：同样降级，不外抛
        let se = search_fragments_impl(&conn, &query("分词检索", Some(sd::SearchMode::Semantic)), None).unwrap();
        assert!(se.degraded);
        assert_eq!(se.items.len(), 1);
    }

    #[test]
    fn rrf_merge_orders_by_both_route_and_normalizes() {
        let keyword = vec![("k1".to_string(), 5.0f64), ("k2".to_string(), 3.0)];
        let semantic = vec![hit("k2", 0.9), hit("s1", 0.8)];
        let rows = rrf_merge(&keyword, &semantic, 10);
        // k2 双路命中 → 名次和最大；k1 关键词第 1 名 > s1 语义第 2 名
        assert_eq!(
            rows.iter().map(|(id, m, _)| (id.as_str(), *m)).collect::<Vec<_>>(),
            vec![("k2", sd::Matched::Both), ("k1", sd::Matched::Keyword), ("s1", sd::Matched::Semantic)]
        );
        assert_eq!(rows[0].2, 1.0); // 归一化到最优命中
        assert!(rows[1].2 > rows[2].2);
        assert!(rows[1].2 < 1.0);
        // limit 在融合后截断
        assert_eq!(rrf_merge(&keyword, &semantic, 2).len(), 2);
    }

    #[test]
    fn hybrid_rrf_prefers_both_route_matches() {
        let conn = crate::db::test_conn();
        let both = submit(&conn, "向量检索与关键词检索混排");
        let only_kw = submit(&conn, "只有关键词检索能命中的一条");
        let only_sem = submit(&conn, "语义相近但字面不含查询词的另一条");
        // 语义路认出 both 与 only_sem；关键词路只认出 both 与 only_kw
        let sem = vec![hit(&both, 0.9), hit(&only_sem, 0.7)];
        let r = search_fragments_impl(&conn, &query("关键词检索", Some(sd::SearchMode::Hybrid)), Some(&sem)).unwrap();
        assert!(!r.degraded);
        assert_eq!(r.items[0].fragment.id, both);
        assert_eq!(r.items[0].matched, sd::Matched::Both);
        assert_eq!(r.items[0].score, 1.0);
        assert_eq!(r.items.len(), 3);
        let rest: Vec<&str> = r.items[1..].iter().map(|h| h.fragment.id.as_str()).collect();
        assert!(rest.contains(&only_kw.as_str()) && rest.contains(&only_sem.as_str()));
        let kinds: Vec<sd::Matched> = r.items[1..].iter().map(|h| h.matched).collect();
        assert!(kinds.contains(&sd::Matched::Semantic) && kinds.contains(&sd::Matched::Keyword));
    }

    #[test]
    fn semantic_mode_ignores_keyword_route_and_empty_is_not_degraded() {
        let conn = crate::db::test_conn();
        let a = submit(&conn, "关键词检索");
        let sem = vec![hit(&a, 0.8)];
        let r = search_fragments_impl(&conn, &query("关键词", Some(sd::SearchMode::Semantic)), Some(&sem)).unwrap();
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.items[0].matched, sd::Matched::Semantic);
        // 可用但库里没向量：不算失败（未降级），semantic 模式即空结果，hybrid 回落关键词序
        let empty: Vec<VectorHit> = vec![];
        let se = search_fragments_impl(&conn, &query("关键词", Some(sd::SearchMode::Semantic)), Some(&empty)).unwrap();
        assert!(!se.degraded);
        assert!(se.items.is_empty());
        let hy = search_fragments_impl(&conn, &query("关键词", None), Some(&empty)).unwrap();
        assert!(!hy.degraded);
        assert_eq!(hy.items.len(), 1);
        assert_eq!(hy.items[0].matched, sd::Matched::Keyword);
    }

    #[test]
    fn semantic_route_drops_soft_deleted() {
        let conn = crate::db::test_conn();
        let gone = submit(&conn, "已经被删掉的向量命中");
        fragments::soft_delete(&conn, &gone).unwrap();
        let r = search_fragments_impl(&conn, &query("向量", Some(sd::SearchMode::Semantic)), Some(&[hit(&gone, 0.9)]))
            .unwrap();
        assert!(r.items.is_empty(), "向量路不得返回已软删条目");
    }

    #[test]
    fn empty_query_rejected() {
        let conn = crate::db::test_conn();
        assert!(matches!(
            search_fragments_impl(&conn, &query("  ", None), None),
            Err(AppError::InputEmpty)
        ));
    }
}
