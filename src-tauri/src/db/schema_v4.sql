-- 迁移 user_version 3→4：批次6-①「原文可编辑」（docs/ux/详情页编辑与常驻操作-设计方案.md）。
-- 目的：让改错的碎片能改回来（此前唯一出路是删除），并留下"改过什么"的账。
-- 两列都是**只加不改**（03 §6 规则）：
--   content_updated_at —— 人工修订正文的时刻，与结果的 processed_at 比较即得「结果已过期」（读时算，不存布尔位）；
--   edit_log           —— JSON 数组的人工修订记录，渲染在详情页原文下方。
-- 沿用 03 §7「本地 <10 万条不过度规范化」的既有取舍：log 用 JSON TEXT，不建 fragment_edits 表。
ALTER TABLE fragments ADD COLUMN content_updated_at TEXT;
ALTER TABLE fragments ADD COLUMN edit_log TEXT NOT NULL DEFAULT '[]';
