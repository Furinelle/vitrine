-- Groups the operator chose to keep; omitted from GET /api/admin/similar.
CREATE TABLE similar_skips (
  group_id TEXT PRIMARY KEY,
  created_at TEXT NOT NULL
);
