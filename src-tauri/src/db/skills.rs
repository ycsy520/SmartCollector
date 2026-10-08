//! skills 表读写（03 §3.9②，02 §7.2）。再加工技能：内置（prompt 只读、不可删）+ 自定义。
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::now_iso;
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub trigger: String,
    pub media_type: String,
    pub category: String,
    pub prompt: String,
    pub enabled: bool,
}

const SKILL_COLS: &str =
    "id, name, builtin, trigger, media_type, category, prompt, enabled";

fn row_to_skill(r: &rusqlite::Row) -> rusqlite::Result<Skill> {
    Ok(Skill {
        id: r.get(0)?,
        name: r.get(1)?,
        builtin: r.get::<_, i64>(2)? != 0,
        trigger: r.get(3)?,
        media_type: r.get(4)?,
        category: r.get(5)?,
        prompt: r.get(6)?,
        enabled: r.get::<_, i64>(7)? != 0,
    })
}

/// 内置技能（PROGRESS V6 + 批次9-P1）：验证真伪 / 创意扩展 / 补全出处。
/// 固定 slug id，prompt 正文与 `04` §6.1 逐字一致（那里是种子文本的唯一来源，此处只是落地副本）。
/// 新增一条内置技能**不需要迁移**：`ensure_builtin` 按 id 逐行 `INSERT OR IGNORE`，老库首启会补插新行
/// （与"改已有 seed 值不生效"是两回事，那才需要一次性 UPDATE，见迁移 5）。
pub(crate) const BUILTINS: &[(&str, &str, &str)] = &[
    (
        "builtin:verify",
        "验证真伪",
        "判断片段中的关键事实陈述是否可信，指出存疑点并给出核查方向。逐条给出结论（可信/存疑/无法判断）与理由。",
    ),
    (
        "builtin:creative",
        "创意扩展",
        "基于片段的核心想法做创意延展，给出 3-5 个可落地的延伸方向，每个方向一句话。",
    ),
    (
        "builtin:source",
        "补全出处",
        "片段若含诗词、古文、歌词、引文等出处性质的文字，补出其作者/朝代/篇名、所引的完整原文、以及白话大意；\
         半句也要给出全篇。不确定出处时一律写「存疑」并说明理由，禁止编造篇名、作者或原文；\
         开头必须写一行「AI 推断，未经核实」。",
    ),
];

/// 建库后确保内置技能存在（幂等，按固定 id 插入，不覆盖用户对 enabled 的开关）。
/// 默认 `enabled=1`：技能入口只有打开时才在详情页出现，关掉等于把"换种方式加工"整条能力藏起来
/// （批次9 实测：两行内置一直 enabled=0，用户从未见过那个按钮）。老库由迁移 5 一次性打开。
pub fn ensure_builtin(conn: &Connection) -> Result<(), AppError> {
    let now = now_iso();
    for (id, name, prompt) in BUILTINS {
        conn.execute(
            "INSERT OR IGNORE INTO skills
               (id, name, builtin, trigger, media_type, category, prompt, enabled, created_at, updated_at)
             VALUES (?1,?2,1,'manual','all','all',?3,1,?4,?4)",
            params![id, name, prompt, &now],
        )
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
    }
    Ok(())
}

pub fn list(conn: &Connection) -> Result<Vec<Skill>, AppError> {
    let mut stmt = conn
        .prepare(&format!("SELECT {SKILL_COLS} FROM skills ORDER BY builtin DESC, created_at"))
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    let rows = stmt
        .query_map([], row_to_skill)
        .map_err(|e| AppError::DbRead(e.to_string()))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| AppError::DbRead(e.to_string()))
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Skill>, AppError> {
    conn.query_row(
        &format!("SELECT {SKILL_COLS} FROM skills WHERE id = ?1"),
        params![id],
        row_to_skill,
    )
    .optional()
    .map_err(|e| AppError::DbRead(e.to_string()))
}

fn validate_trigger(s: &str) -> Result<(), AppError> {
    // 'auto' 在 DDL CHECK 里仍合法（历史行 + 不破坏"只加列不改列"），但 worker 没有任何消费者，
    // 让用户能选出一个"看起来会自动跑、实际永不执行"的技能就是撒谎。批次9 裁定：撤下，不实现。
    if matches!(s, "manual" | "at") {
        Ok(())
    } else if s == "auto" {
        Err(AppError::InputInvalid(
            "trigger: auto（自动执行未实现，请选按钮或 @）".into(),
        ))
    } else {
        Err(AppError::InputInvalid(format!("trigger: {s}")))
    }
}

/// upsert。新建（无 id）→ 生成 UUID，builtin=0。更新已存在的内置技能：**只允许改 `prompt` 与 `enabled`**，
/// name/trigger/media/category 一律沿用库里的值（02 §7.2）——那四项是"这条内置技能是什么"的身份，
/// 改它们等于把内置槽位换成别的东西，`reset_skill` 也就无从"恢复默认"。
pub fn upsert(conn: &Connection, skill: &Skill) -> Result<Skill, AppError> {
    validate_trigger(&skill.trigger)?;
    if skill.name.trim().is_empty() {
        return Err(AppError::InputInvalid("name 不能为空".into()));
    }
    if skill.prompt.trim().is_empty() {
        return Err(AppError::InputInvalid("prompt 不能为空".into()));
    }
    let now = now_iso();
    let existing = get(conn, &skill.id)?;
    match existing.as_ref() {
        Some(cur) if cur.builtin => {
            conn.execute(
                "UPDATE skills SET prompt = ?2, enabled = ?3, updated_at = ?4 WHERE id = ?1",
                params![skill.id, skill.prompt, skill.enabled as i64, &now],
            )
            .map_err(|e| AppError::DbWrite(e.to_string()))?;
        }
        Some(_) => {
            conn.execute(
                "UPDATE skills SET name=?2, trigger=?3, media_type=?4, category=?5, prompt=?6, enabled=?7, updated_at=?8 WHERE id=?1",
                params![skill.id, skill.name, skill.trigger, skill.media_type, skill.category, skill.prompt, skill.enabled as i64, &now],
            )
            .map_err(|e| AppError::DbWrite(e.to_string()))?;
        }
        None => {
            let id = if skill.id.is_empty() { uuid::Uuid::new_v4().to_string() } else { skill.id.clone() };
            conn.execute(
                "INSERT INTO skills (id,name,builtin,trigger,media_type,category,prompt,enabled,created_at,updated_at)
                 VALUES (?1,?2,0,?3,?4,?5,?6,?7,?8,?8)",
                params![id, skill.name, skill.trigger, skill.media_type, skill.category, skill.prompt, skill.enabled as i64, &now],
            )
            .map_err(|e| AppError::DbWrite(e.to_string()))?;
            return get(conn, &id)?.ok_or(AppError::Internal("upsert 后读取失败".into()));
        }
    }
    get(conn, &skill.id)?.ok_or(AppError::NotFound)
}

/// 删除：仅自定义可删；内置返回 StateConflict（02 §7.2）。
pub fn delete(conn: &Connection, id: &str) -> Result<(), AppError> {
    let cur = get(conn, id)?.ok_or(AppError::NotFound)?;
    if cur.builtin {
        return Err(AppError::StateConflict);
    }
    conn.execute("DELETE FROM skills WHERE id = ?1", params![id])
        .map_err(|e| AppError::DbWrite(e.to_string()))?;
    Ok(())
}

/// 恢复内置技能的默认名称与 prompt（改坏了不至于永久回不去）。自定义技能没有"内置默认"，返回 StateConflict。
pub fn reset(conn: &Connection, id: &str) -> Result<Skill, AppError> {
    let cur = get(conn, id)?.ok_or(AppError::NotFound)?;
    if !cur.builtin {
        return Err(AppError::StateConflict);
    }
    let (_, name, prompt) = BUILTINS.iter().find(|(sid, ..)| *sid == id).ok_or(AppError::NotFound)?;
    conn.execute(
        "UPDATE skills SET name = ?2, prompt = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, name, prompt, now_iso()],
    )
    .map_err(|e| AppError::DbWrite(e.to_string()))?;
    get(conn, id)?.ok_or(AppError::Internal("reset 后读取失败".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: &str, name: &str) -> Skill {
        Skill {
            id: id.to_string(),
            name: name.to_string(),
            builtin: false,
            trigger: "manual".into(),
            media_type: "all".into(),
            category: "all".into(),
            prompt: "扩展一下 {{原文}}".into(),
            enabled: true,
        }
    }

    #[test]
    fn ensure_builtin_seeds_all_skills_enabled() {
        let conn = crate::db::test_conn();
        ensure_builtin(&conn).unwrap();
        ensure_builtin(&conn).unwrap(); // 幂等
        let all = list(&conn).unwrap();
        assert_eq!(all.len(), BUILTINS.len());
        assert!(all.iter().all(|s| s.builtin));
        assert!(all.iter().all(|s| s.enabled), "内置技能默认可用，否则再加工入口永远不出现");
    }

    #[test]
    fn custom_skill_crud() {
        let conn = crate::db::test_conn();
        let created = upsert(&conn, &skill("", "我的技能")).unwrap();
        assert!(!created.id.is_empty());
        assert_eq!(created.name, "我的技能");
        // 改 prompt
        let mut upd = created.clone();
        upd.prompt = "改过的模板".into();
        let after = upsert(&conn, &upd).unwrap();
        assert_eq!(after.prompt, "改过的模板");
        // 删除自定义 OK
        delete(&conn, &created.id).unwrap();
        assert!(get(&conn, &created.id).unwrap().is_none());
    }

    #[test]
    fn builtin_prompt_is_editable_and_restorable() {
        let conn = crate::db::test_conn();
        ensure_builtin(&conn).unwrap();
        let seed_prompt = get(&conn, "builtin:verify").unwrap().unwrap().prompt.clone();
        let mut b = get(&conn, "builtin:verify").unwrap().unwrap();
        b.prompt = "篡改".into();
        b.name = "改名".into(); // 身份四项之一：应被忽略
        b.enabled = false;
        let after = upsert(&conn, &b).unwrap();
        assert_eq!(after.prompt, "篡改"); // prompt 可改（用户裁定：内置提示词要能编辑）
        assert_eq!(after.name, "验证真伪"); // name 沿用库里的值
        assert_eq!(after.enabled, false); // enabled 可改
        assert!(matches!(delete(&conn, "builtin:verify"), Err(AppError::StateConflict)));
        // 改坏后可恢复内置默认
        let back = reset(&conn, "builtin:verify").unwrap();
        assert_eq!(back.prompt, seed_prompt);
        assert!(!back.enabled, "reset 只回滚名称与 prompt，不动用户的开关");
        // 自定义技能没有"内置默认"
        let c = upsert(&conn, &skill("", "我的")).unwrap();
        assert!(matches!(reset(&conn, &c.id), Err(AppError::StateConflict)));
    }

    #[test]
    fn validates_trigger_and_required_fields() {
        let conn = crate::db::test_conn();
        let mut bad = skill("", "x");
        bad.trigger = "magic".into();
        assert!(matches!(upsert(&conn, &bad), Err(AppError::InputInvalid(_))));
        let mut noname = skill("", "  ");
        noname.name = "  ".into();
        assert!(matches!(upsert(&conn, &noname), Err(AppError::InputInvalid(_))));
        // 'auto' 在 DDL CHECK 里合法但无人执行：写入侧必须拒，否则用户会养出一个永不跑的技能
        let mut auto = skill("", "自动跑");
        auto.trigger = "auto".into();
        assert!(matches!(upsert(&conn, &auto), Err(AppError::InputInvalid(_))));
    }
}
