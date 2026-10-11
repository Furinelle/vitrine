-- Cached CLIP/hash grouping so GET /api/admin/similar does not recluster 2400 embeddings.
CREATE TABLE similar_cache (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  clip_count INTEGER NOT NULL,
  hash_count INTEGER NOT NULL,
  payload TEXT NOT NULL,
  computed_at TEXT NOT NULL
);
