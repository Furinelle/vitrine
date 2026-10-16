-- Skins (alternate outfits) are their own tags, filed under the character they
-- belong to. SQLite cannot change a CHECK constraint in place, so tag_items is
-- rebuilt with 'skin' added; its triggers are recreated with it.
CREATE TABLE tag_items_new (
  name TEXT PRIMARY KEY,
  kind TEXT NOT NULL DEFAULT 'unsorted'
    CHECK (kind IN ('work', 'character', 'skin', 'cp', 'lore', 'theme', 'medium', 'meta', 'unsorted')),
  parent TEXT,
  ja TEXT,
  en TEXT
);
INSERT INTO tag_items_new (name, kind, parent, ja, en) SELECT name, kind, parent, ja, en FROM tag_items;
DROP TABLE tag_items;
ALTER TABLE tag_items_new RENAME TO tag_items;

CREATE TRIGGER tag_items_insert AFTER INSERT ON tag_items BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER tag_items_update AFTER UPDATE ON tag_items BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER tag_items_delete AFTER DELETE ON tag_items BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
UPDATE similar_generation SET generation=generation+1 WHERE id=1;
