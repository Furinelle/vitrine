-- Similar-image review decisions and recoverable R2 backups.

CREATE TABLE IF NOT EXISTS catalog_prune_receipts (
  decision_id TEXT PRIMARY KEY,
  keep_r2_key TEXT NOT NULL,
  removed_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS catalog_prune_backups (
  original_r2_key TEXT PRIMARY KEY,
  backup_r2_key TEXT NOT NULL,
  decision_id TEXT NOT NULL,
  work_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_catalog_prune_backups_decision
  ON catalog_prune_backups(decision_id);
