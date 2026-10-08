-- 迁移 user_version 5→6：批次17「大模型使用统计」的数据面。
-- 只记元数据：时刻、端点、任务、模型、token 数、成败、错误码、耗时。
--   正文 / prompt / 响应原文 / API Key 一律不落（AGENTS §3；统计面板不得变成第二份原文副本）。
-- 不与 fragments 建外键：碎片 30 天后真删除，账单要留下，否则「删除」会改写历史统计。
-- tokens_in/tokens_out 可空 = 上游没回 usage，与「0 token」是两回事（读侧据此分开显示）。
CREATE TABLE IF NOT EXISTS llm_calls (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  called_at TEXT NOT NULL,
  endpoint TEXT NOT NULL CHECK (endpoint IN ('chat', 'embedding')),
  task TEXT NOT NULL,
  model TEXT NOT NULL,
  tokens_in INTEGER,
  tokens_out INTEGER,
  ok INTEGER NOT NULL CHECK (ok IN (0, 1)),
  error_code TEXT,
  latency_ms INTEGER NOT NULL CHECK (latency_ms >= 0)
);
CREATE INDEX IF NOT EXISTS idx_llm_calls_called_at ON llm_calls(called_at);
