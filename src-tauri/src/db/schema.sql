-- 由 docs/arch/03-SQLite表结构.md §3 转录（唯一 DDL 来源）。
-- 偏离说明（见 docs/PROGRESS.md 待确认项）：
--   fts_fragments 去掉 content='fragments' 外部内容表配置，改为自存储。
--   原因：fts 的 tags 列数据在 processing_results 中，外部内容表无法保存
--   content 表不存在的列；03 §3.8 的隐式跨表触发器在 rusqlite 下不可行。
--   FTS 行由 db 层在 创建片段/写入结果/删除 三处显式同步（03 §3.8 注释中
--   "由应用层显式执行"的同一路径，扩展到全部写点，单一维护者）。
--   03 §5 ① 的 UPDATE...ORDER BY...LIMIT 依赖 SQLITE_ENABLE_UPDATE_DELETE_LIMIT
--   编译选项，改为等价的 IN 子查询写法（jobs.rs）。

-- ============ 3.1 fragments ============
CREATE TABLE IF NOT EXISTS fragments (
  id            TEXT PRIMARY KEY,                -- UUID v4
  content       TEXT NOT NULL,                   -- 原文（或抓取到的正文）
  title         TEXT,                            -- 用户给定或 AI 生成，人工值优先
  title_manual  INTEGER NOT NULL DEFAULT 0,      -- 1=用户手改，retry 不覆盖
  source        TEXT NOT NULL CHECK (source IN ('manual','clipboard','link')),
  content_hash  TEXT NOT NULL,                   -- SHA-256(content)，剪贴板去重用
  external_url  TEXT,                            -- source=link 时的原链接
  char_count    INTEGER NOT NULL,
  created_at    TEXT NOT NULL,                   -- ISO8601 UTC
  updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_frags_created ON fragments(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_frags_hash    ON fragments(content_hash);
CREATE UNIQUE INDEX IF NOT EXISTS uq_frags_active_hash
  ON fragments(content_hash) WHERE content_hash IS NOT NULL;  -- 活跃片段内去重

-- ============ 3.2 fragment_status（处理队列）============
CREATE TABLE IF NOT EXISTS fragment_status (
  fragment_id   TEXT PRIMARY KEY REFERENCES fragments(id) ON DELETE CASCADE,
  status        TEXT NOT NULL CHECK (status IN ('pending','running','done','failed')),
  retry_count   INTEGER NOT NULL DEFAULT 0,
  max_retries   INTEGER NOT NULL DEFAULT 3,
  error_code    TEXT,                            -- 最近一次失败的错误码
  enqueued_at   TEXT NOT NULL,
  started_at    TEXT,
  finished_at   TEXT,
  lease_owner   TEXT,                            -- worker 领取标识，防多 worker 抢同一任务
  lease_expires TEXT                             -- 租约到期后可被重新领取（崩溃恢复）
);
-- worker 取队列的命中索引（部分索引，只索引待办）
CREATE INDEX IF NOT EXISTS idx_status_pending
  ON fragment_status(enqueued_at) WHERE status IN ('pending','running');

-- ============ 3.3 processing_results（AI 结果，版本化）============
CREATE TABLE IF NOT EXISTS processing_results (
  id            TEXT PRIMARY KEY,
  fragment_id   TEXT NOT NULL REFERENCES fragments(id) ON DELETE CASCADE,
  version       INTEGER NOT NULL,                -- 同一片段内递增，最新未废弃版本为准
  superseded    INTEGER NOT NULL DEFAULT 0,      -- 1=被新版本替代（保留供对比/审计）
  category      TEXT NOT NULL DEFAULT '其他',     -- 一级分类，枚举见 04 号文档 §1
  subcategory   TEXT,                            -- 二级分类，可为 NULL
  category_manual  INTEGER NOT NULL DEFAULT 0,
  tags          TEXT NOT NULL DEFAULT '[]',      -- JSON 数组，如 ["Rust","性能优化"]
  tags_manual      INTEGER NOT NULL DEFAULT 0,
  summary       TEXT NOT NULL DEFAULT '',        -- 摘要正文
  links         TEXT NOT NULL DEFAULT '[]',      -- JSON 数组 [{"url","text"}]
  degraded      INTEGER NOT NULL DEFAULT 0,      -- 1=部分任务走了兜底（非完整 AI 结果）
  model_used    TEXT,                            -- 如 "qwen3-max" / "fallback:regex"
  task_meta     TEXT NOT NULL DEFAULT '{}',      -- JSON：每任务耗时/token/是否兜底，调试用
  processed_at  TEXT NOT NULL,
  UNIQUE (fragment_id, version)
);
CREATE INDEX IF NOT EXISTS idx_results_frag ON processing_results(fragment_id, superseded);
CREATE INDEX IF NOT EXISTS idx_results_cat  ON processing_results(category);
-- tags 的查询用 LIKE '%"xx"%' 即可（本地量级 <10万），不建 JSON 索引

-- ============ 3.4 vector_refs（向量映射主表）============
-- 实际向量存 sqlite-vec 虚表 vec_embeddings；本表管生命周期与版本一致性。
-- vec_embeddings 虚表按当前 config 的 dim 在建库/换模型时由 vector 模块创建（P6），不入本迁移。
CREATE TABLE IF NOT EXISTS vector_refs (
  fragment_id   TEXT PRIMARY KEY REFERENCES fragments(id) ON DELETE CASCADE,
  result_version INTEGER NOT NULL,               -- 对应 processing_results.version
  model         TEXT NOT NULL,                   -- embedding 模型名，换模型需全量重建
  dim           INTEGER NOT NULL,
  embedded_at   TEXT NOT NULL
);

-- ============ 3.5 config（KV）============
CREATE TABLE IF NOT EXISTS config (
  key        TEXT PRIMARY KEY,                   -- 点分命名，见 §4 默认键
  value      TEXT NOT NULL,                      -- JSON 编码的值（字符串也带引号）
  updated_at TEXT NOT NULL
);

-- ============ 3.6 deletions（墓碑）============
CREATE TABLE IF NOT EXISTS deletions (
  fragment_id TEXT PRIMARY KEY,
  deleted_at  TEXT NOT NULL
);

-- ============ 3.7 fts_fragments（关键词搜索，自存储，见文件头偏离说明）============
-- 中文检索用 trigram（03 §3.7 注 a)：rusqlite bundled SQLite ≥3.34 满足）。
CREATE VIRTUAL TABLE IF NOT EXISTS fts_fragments USING fts5(
  title, content, tags,
  tokenize='trigram'
);
