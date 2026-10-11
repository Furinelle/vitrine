-- Perceptual fingerprints computed on Oracle, stored for Worker-side clustering.
CREATE TABLE image_fingerprints (
  r2_key TEXT PRIMARY KEY,
  work_id TEXT NOT NULL,
  solid_color INTEGER NOT NULL DEFAULT 0,
  content_sha256 TEXT NOT NULL,
  strict_key TEXT NOT NULL,
  average_hash TEXT NOT NULL,
  difference_hash TEXT NOT NULL,
  color_key TEXT NOT NULL,
  detail_key TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  format TEXT NOT NULL,
  regions_json TEXT NOT NULL DEFAULT '[]',
  updated_at TEXT NOT NULL
);

CREATE INDEX idx_image_fingerprints_work ON image_fingerprints(work_id);
