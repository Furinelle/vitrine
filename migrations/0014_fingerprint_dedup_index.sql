-- Ingest-time strict duplicate lookup (see similar::dedup_image).
CREATE INDEX IF NOT EXISTS idx_image_fingerprints_sha ON image_fingerprints(content_sha256);
CREATE INDEX IF NOT EXISTS idx_image_fingerprints_visual ON image_fingerprints(color_key, detail_key);
