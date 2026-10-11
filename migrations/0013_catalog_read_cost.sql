-- Range pagination and source filters must not sort/scan the full catalog.
CREATE INDEX idx_works_visible_created ON works(created_at DESC,id DESC) WHERE deleted_at IS NULL;
CREATE INDEX idx_works_visible_source_created ON works(source,created_at DESC,id DESC) WHERE deleted_at IS NULL;

-- Public tag statistics also change when tag associations are edited directly.
CREATE TRIGGER similar_work_tags_insert AFTER INSERT ON work_tags BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_work_tags_delete AFTER DELETE ON work_tags BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_work_tags_update AFTER UPDATE ON work_tags
WHEN OLD.work_id IS NOT NEW.work_id OR OLD.tag IS NOT NEW.tag BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;

-- Discard filters cached before direct association writes were versioned.
UPDATE similar_generation SET generation=generation+1 WHERE id=1;
