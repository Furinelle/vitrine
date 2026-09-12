-- Whole-work changes invalidate old per-image undo snapshots.
ALTER TABLE works ADD COLUMN review_version INTEGER NOT NULL DEFAULT 0;

-- Public media authorization looks up the current image by its immutable key.
CREATE INDEX IF NOT EXISTS idx_images_r2_key ON images(r2_key);
