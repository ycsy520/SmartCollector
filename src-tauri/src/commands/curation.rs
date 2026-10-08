//! 分拣辅助 command（02 §7.2 Skill / §7.3 Habit / §7.4 火花回路）。
//! run_skill（§2.7）在 commands/fragment.rs；此处 §7.4 两条：list_related 走向量 embedding
//! 余弦近邻、无向量时回退 bigram Dice（阈值 0.15 仍在 db 层），get_week_digest 纯计量推导不接 LLM。
use rusqlite::Connection;
use tauri::State;

use super::fragment::{build_detail, to_summary};
use crate::db::{config_store, fragments, habits, skills};
use crate::dto::fragment::{HabitRule, RelatedHit, RelatedOutcome, Skill, WeekDigest};
use crate::error::AppError;
use crate::state::AppState;
use crate::vector::semantic::SemanticIndex;
use crate::vector::VectorHit;

/// 向量近邻的相似度下限（余弦 ∈ [-1,1]，实测多在 0.3~0.9）。取值未经真数据校准，见文件头注释。
const MIN_SIM: f32 = 0.30;

pub(crate) fn list_skills_impl(conn: &Connection) -> Result<Vec<Skill>, AppError> {
    skills::list(conn)
}

pub(crate) fn save_skill_impl(conn: &Connection, skill: &Skill) -> Result<Skill, AppError> {
    skills::upsert(conn, skill)
}

pub(crate) fn delete_skill_impl(conn: &Connection, id: &str) -> Result<serde_json::Value, AppError> {
    skills::delete(conn, id)?;
    Ok(serde_json::json!({ "deleted": true }))
}

/// 内置技能恢复默认（名称 + prompt）。自定义技能无默认 → StateConflict。
pub(crate) fn reset_skill_impl(conn: &Connection, id: &str) -> Result<Skill, AppError> {
    skills::reset(conn, id)
}

pub(crate) fn list_habits_impl(conn: &Connection) -> Result<Vec<HabitRule>, AppError> {
    habits::list(conn)
}

pub(crate) fn set_habit_state_impl(conn: &Connection, id: &str, state: &str) -> Result<HabitRule, AppError> {
    habits::set_state(conn, id, state)
}

/// 02 §7.4 相关碎片：优先向量 embedding 余弦近邻（库里已存的向量，不再花一次 embedding 调用），
/// 语义路不可用或一个都没过阈值时回退 bigram Dice（契约明示的兜底算法）。两条路都排除自身与垃圾站。
pub(crate) fn list_related_impl(
    conn: &Connection,
    fragment_id: &str,
    limit: Option<i64>,
    semantic: Option<&[VectorHit]>,
) -> Result<RelatedOutcome, AppError> {
    let k = limit.unwrap_or(5).clamp(1, 20) as usize;
    let mut items = Vec::new();
    if let Some(hits) = semantic {
        // 相似度下限：KNN 永远返回 k 个"最近的"，不设闸的话收藏一多就总把无关条目当相关推荐（噪声）。
        // 0.30 是未经验证的保守取值，真 embedding 上线后按实测调（见 待确认清单）。
        let live = crate::vector::semantic::live_hits(conn, hits, &["trash"])?;
        for hit in live.iter().filter(|h| h.fragment_id != fragment_id && h.score >= MIN_SIM).take(k) {
            items.push(RelatedHit {
                fragment: to_summary(&build_detail(conn, &hit.fragment_id)?),
                score: hit.score as f64,
            });
        }
    }
    if items.is_empty() {
        for (id, score) in fragments::related_by_dice(conn, fragment_id, k)? {
            items.push(RelatedHit { fragment: to_summary(&build_detail(conn, &id)?), score });
        }
    }
    Ok(RelatedOutcome { items })
}

/// 导语只复述计量，不外推（02 §7.4「insight 非编造」）。
fn week_insight(a: &fragments::WeekActivity) -> String {
    if a.sorted + a.trashed + a.auto_archived == 0 {
        return if a.pending > 0 {
            format!("这一周还没有分拣动作，待分拣里攒着 {} 条。", a.pending)
        } else {
            "这一周还没有分拣动作。".into()
        };
    }
    let auto = if a.auto_archived > 0 { format!("，另有 {} 条超时替你归档", a.auto_archived) } else { String::new() };
    let mut s = format!("这一周你放行 {} 条、丢弃 {} 条{}。", a.sorted, a.trashed, auto);
    if let Some((cat, n)) = &a.top_category {
        s.push_str(&format!("归档最多的是「{cat}」（{n} 条）。"));
    }
    if a.pending > 0 {
        s.push_str(&format!("待分拣还有 {} 条。", a.pending));
    }
    s
}

pub(crate) fn get_week_digest_impl(conn: &Connection) -> Result<WeekDigest, AppError> {
    let a = fragments::week_activity(conn)?;
    Ok(WeekDigest {
        sorted: a.sorted,
        trashed: a.trashed,
        auto_archived: a.auto_archived,
        pending: a.pending,
        insight: week_insight(&a),
    })
}

macro_rules! cur {
    ($app:expr, $f:expr) => {{
        let conn = $app.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
        ($f)(&conn)
    }};
}

#[tauri::command]
pub fn list_skills(app: State<'_, AppState>) -> Result<Vec<Skill>, AppError> {
    cur!(app, |c: &Connection| list_skills_impl(c))
}

#[tauri::command]
pub fn save_skill(app: State<'_, AppState>, skill: Skill) -> Result<Skill, AppError> {
    cur!(app, |c: &Connection| save_skill_impl(c, &skill))
}

#[tauri::command]
pub fn delete_skill(app: State<'_, AppState>, skill_id: String) -> Result<serde_json::Value, AppError> {
    cur!(app, |c: &Connection| delete_skill_impl(c, &skill_id))
}

#[tauri::command]
pub fn reset_skill(app: State<'_, AppState>, skill_id: String) -> Result<Skill, AppError> {
    cur!(app, |c: &Connection| reset_skill_impl(c, &skill_id))
}

#[tauri::command]
pub fn list_habits(app: State<'_, AppState>) -> Result<Vec<HabitRule>, AppError> {
    cur!(app, |c: &Connection| list_habits_impl(c))
}

#[tauri::command]
pub fn set_habit_state(app: State<'_, AppState>, habit_id: String, state: String) -> Result<HabitRule, AppError> {
    cur!(app, |c: &Connection| set_habit_state_impl(c, &habit_id, &state))
}

#[tauri::command]
pub fn list_related(
    state: State<'_, AppState>,
    fragment_id: String,
    limit: Option<i64>,
) -> Result<RelatedOutcome, AppError> {
    let conn = state.db.get().map_err(|e| AppError::DbRead(e.to_string()))?;
    let k = limit.unwrap_or(5).clamp(1, 20) as usize;
    // 语义路只读库里已存的向量（无网络调用，故不必另起线程）；未入向量库 → None → Dice 兜底。
    let cfg = config_store::load(&conn)?;
    let semantic = SemanticIndex::open(&state.db, &cfg)
        .and_then(|idx| crate::vector::semantic::near_fragment(idx.store(), &fragment_id, k).ok());
    list_related_impl(&conn, &fragment_id, limit, semantic.as_deref())
}

#[tauri::command]
pub fn get_week_digest(app: State<'_, AppState>) -> Result<WeekDigest, AppError> {
    cur!(app, |c: &Connection| get_week_digest_impl(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_crud_with_builtin_gating() {
        let conn = crate::db::test_conn();
        skills::ensure_builtin(&conn).unwrap();
        assert_eq!(list_skills_impl(&conn).unwrap().len(), skills::BUILTINS.len());
        let custom = Skill {
            id: String::new(), name: "自定义".into(), builtin: false, trigger: "manual".into(),
            media_type: "all".into(), category: "all".into(), prompt: "做点什么".into(), enabled: false,
        };
        let saved = save_skill_impl(&conn, &custom).unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(list_skills_impl(&conn).unwrap().len(), skills::BUILTINS.len() + 1);
        assert_eq!(delete_skill_impl(&conn, &saved.id).unwrap()["deleted"], true);
        // 内置不可删
        assert!(matches!(delete_skill_impl(&conn, "builtin:verify"), Err(AppError::StateConflict)));
    }

    #[test]
    fn habits_list_and_state() {
        let conn = crate::db::test_conn();
        habits::upsert_candidate(&conn, "h1", "资讯×链接", "丢弃", 3, 4, &[]).unwrap();
        assert_eq!(list_habits_impl(&conn).unwrap()[0].state, "candidate");
        let a = set_habit_state_impl(&conn, "h1", "active").unwrap();
        assert_eq!(a.state, "active");
        assert!(matches!(set_habit_state_impl(&conn, "h1", "bogus"), Err(AppError::InputInvalid(_))));
    }

    use crate::commands::fragment::{set_fragment_layer_impl, submit_text_impl};
    use crate::dto::fragment::SubmitInput;

    fn submit(conn: &Connection, text: &str) -> String {
        submit_text_impl(conn, &SubmitInput { content: text.into(), title: None, note: None, skill_ids: vec![] })
            .unwrap()
            .fragment_id
    }

    #[test]
    fn related_finds_near_duplicate_and_excludes_trash() {
        let conn = crate::db::test_conn();
        let base = "Material 3 的 tonal palette 用同色相的明度阶梯组织浅深两层配色";
        let target = submit(&conn, base);
        let twin = submit(&conn, &format!("{base}，暗色靠 elevation 抬升而不是更黑的背景"));
        submit(&conn, "空印案牵连数千人，胡惟庸案后朕欲整饬吏治以安民心");
        let r = list_related_impl(&conn, &target, None, None).unwrap();
        assert_eq!(r.items.len(), 1, "只应命中近重复那条");
        assert_eq!(r.items[0].fragment.id, twin);
        assert!(r.items[0].score >= 0.15);
        // 垃圾站里的同文不再算相关
        set_fragment_layer_impl(&conn, &twin, "trash").unwrap();
        assert!(list_related_impl(&conn, &target, None, None).unwrap().items.is_empty());
    }

    #[test]
    fn related_prefers_vector_hits_and_gates_trash_and_threshold() {
        let conn = crate::db::test_conn();
        let target = submit(&conn, "向量近邻的目标条目，字面与另一条完全不同");
        let near = submit(&conn, "红烧肉先焯水再慢炖收汁的家常做法");
        let cold = submit(&conn, "语义相似度低于阈值的一条向量命中记录");
        let gone = submit(&conn, "已进垃圾站但向量仍在库里的一条");
        set_fragment_layer_impl(&conn, &gone, "trash").unwrap();
        let sem = |id: &str, score: f32| VectorHit { fragment_id: id.into(), score };
        let hits = vec![
            sem(&gone, 0.95),
            sem(&near, 0.72),
            sem(&target, 1.0), // 自身：契约要求排除，向量路也得剔掉
            sem(&cold, 0.21), // 低于 MIN_SIM
        ];
        let r = list_related_impl(&conn, &target, Some(5), Some(&hits)).unwrap();
        assert_eq!(r.items.len(), 1, "只留过阈值且可见的那条");
        assert_eq!(r.items[0].fragment.id, near);
        assert!((r.items[0].score - 0.72).abs() < 1e-6, "f32 余弦升 f64 有末位误差");
        // 语义路过阈值数为 0 → 回落 Dice，而不是返回空
        let weak = vec![sem(&near, 0.10)];
        let fb = list_related_impl(&conn, &target, Some(5), Some(&weak)).unwrap();
        assert!(fb.items.iter().all(|h| h.fragment.id != near), "Dice 不该照抄语义命中");
        let none = list_related_impl(&conn, &target, Some(5), None).unwrap();
        assert_eq!(none.items.len(), fb.items.len(), "无语义路与弱语义路应同为 Dice 结果");
    }

    #[test]
    fn week_digest_counts_actions_and_names_top_category() {
        let conn = crate::db::test_conn();
        let keep = submit(&conn, "本周被人工放行的一条正常长度内容");
        let gone = submit(&conn, "本周被丢进垃圾站的一条正常长度内容");
        crate::db::results::insert_result(&conn, &keep, "技术", None, &[], "摘要", &[], false, None).unwrap();
        set_fragment_layer_impl(&conn, &keep, "archived").unwrap();
        set_fragment_layer_impl(&conn, &gone, "trash").unwrap();
        let d = get_week_digest_impl(&conn).unwrap();
        assert_eq!((d.sorted, d.trashed, d.auto_archived, d.pending), (1, 1, 0, 0));
        assert!(d.insight.contains("放行 1 条"), "{}", d.insight);
        assert!(d.insight.contains("技术"), "{}", d.insight);
    }

    #[test]
    fn week_digest_on_empty_db_says_so() {
        let conn = crate::db::test_conn();
        let d = get_week_digest_impl(&conn).unwrap();
        assert_eq!((d.sorted, d.trashed, d.auto_archived, d.pending), (0, 0, 0, 0));
        assert_eq!(d.insight, "这一周还没有分拣动作。");
    }
}
