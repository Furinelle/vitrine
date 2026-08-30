-- Telegram channel publication mappings and whole-work prune receipts.

CREATE TABLE telegram_publications (
  id TEXT PRIMARY KEY,
  work_id TEXT NOT NULL,
  chat_id INTEGER NOT NULL,
  anchor_message_id INTEGER NOT NULL,
  message_ids_json TEXT NOT NULL,
  publish_state TEXT NOT NULL CHECK (publish_state IN ('full','partial')),
  created_at TEXT NOT NULL,
  deleted_at TEXT,
  UNIQUE(work_id, chat_id, anchor_message_id)
);
CREATE INDEX idx_telegram_publications_work_active ON telegram_publications(work_id, deleted_at);
CREATE TABLE catalog_work_prune_receipts (
  decision_id TEXT PRIMARY KEY,
  keep_work_id TEXT NOT NULL,
  remove_work_ids_json TEXT NOT NULL,
  removed_r2_keys_json TEXT NOT NULL,
  telegram_targets_json TEXT NOT NULL,
  telegram_state TEXT NOT NULL CHECK (telegram_state IN ('pending','complete')),
  telegram_error TEXT,
  created_at TEXT NOT NULL,
  telegram_completed_at TEXT
);
CREATE INDEX idx_catalog_work_prune_telegram_state ON catalog_work_prune_receipts(telegram_state, created_at);
