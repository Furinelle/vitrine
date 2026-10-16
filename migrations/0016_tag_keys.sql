-- Duplicate-tag matching: tag_key(tag) (case, width, punctuation, invisible
-- characters and Traditional/Simplified folded) -> the one stored spelling.
-- Ingest and admin edits insert the first spelling seen for a key and store
-- every later variant under it. Existing tags are keyed by tools/tag_merge.py.
CREATE TABLE tag_keys (
  key TEXT PRIMARY KEY,
  tag TEXT NOT NULL
);
