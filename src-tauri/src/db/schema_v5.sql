-- 迁移 user_version 4→5：批次9-P0c「接通再加工技能」——一次性把内置技能的开关打开。
-- 背景：`ensure_builtin` 的 seed 一直写 `enabled=0`，而详情页「换个方向再加工」chip 的出现条件
-- 是"存在 enabled 且门控命中的技能"——于是内置的「验证真伪」「创意扩展」自落地起从未在界面上出现过，
-- 用户看到的能力集合里根本没有"换种方式加工"这一项（本机库实测：2 行 builtin 全 enabled=0）。
-- 只动数据、不动 DDL：'auto' 这个 trigger 值在 skills 表的 CHECK 里仍然合法（改 CHECK 要重建表，
-- 违反 03 §6「只加列不改列」），但它在 `db/skills.rs::validate_trigger` 一侧已被拒绝——
-- 历史行照旧可读，新写入不再可能产出一个"看着会自动跑、实际永不执行"的技能。
UPDATE skills SET enabled = 1, updated_at = updated_at WHERE builtin = 1 AND enabled = 0;
