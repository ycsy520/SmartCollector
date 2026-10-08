-- 迁移 user_version 1→2：UI v2 三层分拣 / 再加工 / 习惯 存储落地。
-- 转录自 docs/arch/03-SQLite表结构.md §3.9（唯一 DDL 来源，"只加列不改列"）。
-- ALTER TABLE ADD COLUMN 非幂等，故整体包在事务内：中途失败则回滚、user_version 保持 1，可安全重跑。
BEGIN;

-- ① fragments 加分拣层与附言字段
ALTER TABLE fragments ADD COLUMN layer       TEXT NOT NULL DEFAULT 'buffer'
  CHECK (layer IN ('buffer','archived','trash'));         -- 缓冲区/归档/垃圾站
ALTER TABLE fragments ADD COLUMN media_type  TEXT NOT NULL DEFAULT 'text'
  CHECK (media_type IN ('text','link','image'));          -- 信息类型（与分类正交）
ALTER TABLE fragments ADD COLUMN archived_by TEXT          -- 'manual' | 'auto' | NULL
  CHECK (archived_by IS NULL OR archived_by IN ('manual','auto'));
ALTER TABLE fragments ADD COLUMN reviewed    INTEGER NOT NULL DEFAULT 0; -- 1=经人工分拣
ALTER TABLE fragments ADD COLUMN trashed_at  TEXT;         -- 进垃圾站时刻（30 天硬删基准，UTC）
ALTER TABLE fragments ADD COLUMN note        TEXT;         -- 人工附言，参与检索

CREATE INDEX IF NOT EXISTS idx_frags_layer ON fragments(layer, created_at DESC);

-- 去重唯一索引改为只在"非垃圾站"活跃片段内生效（原 uq_frags_active_hash 含 trash 会误拦重收集）
DROP INDEX IF EXISTS uq_frags_active_hash;
CREATE UNIQUE INDEX IF NOT EXISTS uq_frags_active_hash
  ON fragments(content_hash) WHERE content_hash IS NOT NULL AND layer <> 'trash';

-- ② skills（再加工技能，内置 + 自定义）
CREATE TABLE IF NOT EXISTS skills (
  id         TEXT PRIMARY KEY,               -- 内置用固定 slug，自定义 UUID v4
  name       TEXT NOT NULL,
  builtin    INTEGER NOT NULL DEFAULT 0,     -- 1=内置（不可删、prompt 只读，仅可 enabled 开关）
  trigger    TEXT NOT NULL CHECK (trigger IN ('auto','manual','at')),
  media_type TEXT NOT NULL DEFAULT 'all',    -- 'all' 或 MediaType
  category   TEXT NOT NULL DEFAULT 'all',    -- 'all' 或一级分类
  prompt     TEXT NOT NULL,                  -- 模板，含 {{原文}} 占位（04 §6）
  enabled    INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- ③ habits（分拣习惯：AI 归纳的建议，须用户显式启用）
CREATE TABLE IF NOT EXISTS habits (
  id          TEXT PRIMARY KEY,
  pattern     TEXT NOT NULL,                 -- 条件摘要（AI 生成的人话）
  action      TEXT NOT NULL,                 -- 丢弃 / 快速归档 / @skill:{id}
  hits        INTEGER NOT NULL DEFAULT 0,    -- 同类人工动作次数
  total       INTEGER NOT NULL DEFAULT 0,    -- 观察样本数
  state       TEXT NOT NULL DEFAULT 'candidate'
                CHECK (state IN ('candidate','active','paused')),
  recent_auto INTEGER NOT NULL DEFAULT 0,    -- active 态：近 7 天自动处理数
  samples     TEXT NOT NULL DEFAULT '[]',    -- JSON 数组：证据样本摘录
  updated_at  TEXT NOT NULL
);

COMMIT;
