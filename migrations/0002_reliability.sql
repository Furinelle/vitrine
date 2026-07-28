-- Phase 3: additive reliability schema for versioned ingest / GC / idempotency.
-- Safe to apply after 0001_init.sql. Does not rewrite existing rows beyond defaults.

-- Soft-delete marker for works (list APIs should treat non-null as hidden).
ALTER TABLE works ADD COLUMN deleted_at TEXT;

-- Per-image content digest for integrity / fingerprinting.
ALTER TABLE images ADD COLUMN sha256 TEXT NOT NULL DEFAULT '';

-- Idempotency-Key receipts: same key + fingerprint → replay; conflict → 409.
CREATE TABLE IF NOT EXISTS idempotency_receipts (
  idempotency_key TEXT PRIMARY KEY,
  fingerprint TEXT NOT NULL,
  work_id TEXT NOT NULL,
  status_code INTEGER NOT NULL DEFAULT 200,
  response_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_idempotency_receipts_work
  ON idempotency_receipts(work_id);
CREATE INDEX IF NOT EXISTS idx_idempotency_receipts_created
  ON idempotency_receipts(created_at DESC);

-- Append-only ingest audit trail.
CREATE TABLE IF NOT EXISTS ingest_audits (
  id TEXT PRIMARY KEY,
  work_id TEXT NOT NULL,
  source TEXT NOT NULL,
  source_id TEXT NOT NULL,
  ingest_id TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  idempotency_key TEXT,
  page_count INTEGER NOT NULL DEFAULT 0,
  total_bytes INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL,
  error TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ingest_audits_work ON ingest_audits(work_id);
CREATE INDEX IF NOT EXISTS idx_ingest_audits_created ON ingest_audits(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_ingest_audits_status ON ingest_audits(status);

-- Old / failed R2 objects recorded for later GC (never deleted inline on success path).
CREATE TABLE IF NOT EXISTS orphan_objects (
  r2_key TEXT PRIMARY KEY,
  work_id TEXT NOT NULL,
  reason TEXT NOT NULL DEFAULT 'replaced',
  created_at TEXT NOT NULL,
  deleted_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_orphan_objects_work ON orphan_objects(work_id);
CREATE INDEX IF NOT EXISTS idx_orphan_objects_pending
  ON orphan_objects(deleted_at, created_at);
