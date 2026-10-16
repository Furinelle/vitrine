-- Tags removed as meaningless (platform campaigns, commission notices, overly
-- broad words, memes). Rows are written by tools/tag_merge.py from
-- tools/tag_fixes.json `drop`; variants reach the blocked name through
-- tag_keys. A blocked tag written by ingest, admin edits or direct SQL is
-- skipped before it is stored (RAISE(IGNORE)), so no AFTER trigger such as the
-- tag_counts or tag_links ones ever sees it.
CREATE TABLE tag_blocks (
  name TEXT PRIMARY KEY
);
CREATE TRIGGER tag_blocks_work_tags_insert BEFORE INSERT ON work_tags
WHEN EXISTS (SELECT 1 FROM tag_blocks WHERE name = NEW.tag) BEGIN
  SELECT RAISE(IGNORE);
END;
CREATE TRIGGER tag_blocks_tags_insert BEFORE INSERT ON tags
WHEN EXISTS (SELECT 1 FROM tag_blocks WHERE name = NEW.name) BEGIN
  SELECT RAISE(IGNORE);
END;
