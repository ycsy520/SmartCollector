-- 迁移 user_version 2→3：批次2 入队前闸门引入 'skipped' 状态。
-- 目的：零 LLM token 把「垃圾/仅记录」片段直接落 skipped，worker 永不领取（claim 只取 pending）。
-- SQLite 不能就地放宽 CHECK，沿用「重建表 + 迁数据 + 改名 + 重建索引」标准流程；
-- 迁移按 user_version 只执行一次，且数据全列同名，故幂等且无损。
PRAGMA foreign_keys=OFF;

CREATE TABLE fragment_status_new (
  fragment_id   TEXT PRIMARY KEY REFERENCES fragments(id) ON DELETE CASCADE,
  status        TEXT NOT NULL CHECK (status IN ('pending','running','done','failed','skipped')),
  retry_count   INTEGER NOT NULL DEFAULT 0,
  max_retries   INTEGER NOT NULL DEFAULT 3,
  error_code    TEXT,
  enqueued_at   TEXT NOT NULL,
  started_at    TEXT,
  finished_at   TEXT,
  lease_owner   TEXT,
  lease_expires TEXT
);

INSERT INTO fragment_status_new
      (fragment_id, status, retry_count, max_retries, error_code, enqueued_at,
       started_at, finished_at, lease_owner, lease_expires)
SELECT fragment_id, status, retry_count, max_retries, error_code, enqueued_at,
       started_at, finished_at, lease_owner, lease_expires
  FROM fragment_status;

DROP TABLE fragment_status;
ALTER TABLE fragment_status_new RENAME TO fragment_status;

CREATE INDEX IF NOT EXISTS idx_status_pending
  ON fragment_status(enqueued_at) WHERE status IN ('pending','running');

PRAGMA foreign_keys=ON;
