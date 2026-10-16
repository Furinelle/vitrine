-- Tag library: what a stored tag (work_tags.tag) is. A row exists only for
-- curated tags; any other tag is 'unsorted'. For a linked group the stored name
-- is the Chinese one and `ja` / `en` hold the other display names (their
-- spellings resolve to the stored name through tag_keys). `parent` is the work
-- a character or setting belongs to. Rows are written by tools/tag_merge.py.
CREATE TABLE tag_items (
  name TEXT PRIMARY KEY,
  kind TEXT NOT NULL DEFAULT 'unsorted'
    CHECK (kind IN ('work', 'character', 'cp', 'lore', 'theme', 'medium', 'meta', 'unsorted')),
  parent TEXT,
  ja TEXT,
  en TEXT
);

-- The cached public tag list carries these fields, so any change refreshes it.
CREATE TRIGGER tag_items_insert AFTER INSERT ON tag_items BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER tag_items_update AFTER UPDATE ON tag_items BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER tag_items_delete AFTER DELETE ON tag_items BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
