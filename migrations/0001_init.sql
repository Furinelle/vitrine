-- Vitrine gallery schema
CREATE TABLE IF NOT EXISTS works (
  id TEXT PRIMARY KEY,
  source TEXT NOT NULL,          -- pixiv | x | douyin
  source_id TEXT NOT NULL,
  source_url TEXT NOT NULL DEFAULT '',
  title TEXT NOT NULL DEFAULT '',
  author_name TEXT NOT NULL DEFAULT '',
  author_url TEXT NOT NULL DEFAULT '',
  is_r18 INTEGER NOT NULL DEFAULT 0,
  page_count INTEGER NOT NULL DEFAULT 1,
  origin TEXT NOT NULL DEFAULT '',  -- hanabi 源实例名
  created_at TEXT NOT NULL,
  UNIQUE(source, source_id)
);

CREATE TABLE IF NOT EXISTS images (
  id TEXT PRIMARY KEY,
  work_id TEXT NOT NULL,
  page_index INTEGER NOT NULL DEFAULT 0,
  r2_key TEXT NOT NULL,
  content_type TEXT NOT NULL DEFAULT 'image/jpeg',
  byte_size INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  FOREIGN KEY (work_id) REFERENCES works(id) ON DELETE CASCADE,
  UNIQUE(work_id, page_index)
);

CREATE TABLE IF NOT EXISTS tags (
  name TEXT PRIMARY KEY,
  use_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS work_tags (
  work_id TEXT NOT NULL,
  tag TEXT NOT NULL,
  PRIMARY KEY (work_id, tag),
  FOREIGN KEY (work_id) REFERENCES works(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_works_source ON works(source);
CREATE INDEX IF NOT EXISTS idx_works_created ON works(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_work_tags_tag ON work_tags(tag);
CREATE INDEX IF NOT EXISTS idx_images_work ON images(work_id);
