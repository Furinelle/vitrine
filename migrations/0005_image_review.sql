-- Preserve exact image metadata and channel targets for per-image undo.
CREATE TABLE catalog_image_reviews (
  decision_id TEXT PRIMARY KEY,
  payload TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('prepared','deleted','restored')),
  created_at TEXT NOT NULL
);
