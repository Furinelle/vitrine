-- Oracle cold-store archiver pages gallery originals oldest first.
CREATE INDEX IF NOT EXISTS idx_images_created ON images(created_at, id);
