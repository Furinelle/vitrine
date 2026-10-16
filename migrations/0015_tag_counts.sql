-- Public tag statistics read one row per tag instead of joining every
-- work_tags row with works on each filter-cache miss.
CREATE TABLE tag_counts (
  tag TEXT PRIMARY KEY,
  cnt INTEGER NOT NULL
);
INSERT INTO tag_counts (tag, cnt)
  SELECT wt.tag, COUNT(*) FROM work_tags wt JOIN works w ON w.id = wt.work_id
  WHERE w.deleted_at IS NULL GROUP BY wt.tag;

-- Only associations of visible (not soft-deleted) works are counted.
CREATE TRIGGER tag_counts_work_tags_insert AFTER INSERT ON work_tags
WHEN EXISTS (SELECT 1 FROM works WHERE id = NEW.work_id AND deleted_at IS NULL) BEGIN
  INSERT INTO tag_counts (tag, cnt) VALUES (NEW.tag, 1)
  ON CONFLICT(tag) DO UPDATE SET cnt = cnt + 1;
END;
CREATE TRIGGER tag_counts_work_tags_delete AFTER DELETE ON work_tags
WHEN EXISTS (SELECT 1 FROM works WHERE id = OLD.work_id AND deleted_at IS NULL) BEGIN
  UPDATE tag_counts SET cnt = cnt - 1 WHERE tag = OLD.tag;
END;
CREATE TRIGGER tag_counts_work_tags_update AFTER UPDATE ON work_tags
WHEN OLD.work_id IS NOT NEW.work_id OR OLD.tag IS NOT NEW.tag BEGIN
  UPDATE tag_counts SET cnt = cnt - 1
  WHERE tag = OLD.tag AND EXISTS (SELECT 1 FROM works WHERE id = OLD.work_id AND deleted_at IS NULL);
  INSERT INTO tag_counts (tag, cnt)
  SELECT NEW.tag, 1 WHERE EXISTS (SELECT 1 FROM works WHERE id = NEW.work_id AND deleted_at IS NULL)
  ON CONFLICT(tag) DO UPDATE SET cnt = cnt + 1;
END;

-- Soft delete and restore move every tag of the work out of / into the counts.
CREATE TRIGGER tag_counts_work_hidden AFTER UPDATE OF deleted_at ON works
WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL BEGIN
  UPDATE tag_counts SET cnt = cnt - 1
  WHERE tag IN (SELECT tag FROM work_tags WHERE work_id = NEW.id);
END;
-- A hard delete cascades to work_tags after the works row is gone, where the
-- delete trigger above no longer sees it as visible; count it out first.
CREATE TRIGGER tag_counts_work_deleted BEFORE DELETE ON works
WHEN OLD.deleted_at IS NULL BEGIN
  UPDATE tag_counts SET cnt = cnt - 1
  WHERE tag IN (SELECT tag FROM work_tags WHERE work_id = OLD.id);
END;
CREATE TRIGGER tag_counts_work_shown AFTER UPDATE OF deleted_at ON works
WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NULL BEGIN
  INSERT INTO tag_counts (tag, cnt)
  SELECT tag, 1 FROM work_tags WHERE work_id = NEW.id
  ON CONFLICT(tag) DO UPDATE SET cnt = cnt + 1;
END;
