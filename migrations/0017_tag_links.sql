-- Bilingual game tags: a work carrying `tag` also carries `also` (each game's
-- Chinese title and English name link both ways). Rows are written by
-- tools/tag_merge.py from tools/tag_aliases.json; the trigger covers ingest,
-- admin edits and direct writes. INSERT OR IGNORE stops at the pair, so the
-- companion's own link back to the original inserts nothing.
CREATE TABLE tag_links (
  tag TEXT NOT NULL,
  also TEXT NOT NULL,
  PRIMARY KEY (tag, also)
);
CREATE TRIGGER tag_links_work_tags_insert AFTER INSERT ON work_tags BEGIN
  INSERT OR IGNORE INTO work_tags (work_id, tag)
  SELECT NEW.work_id, also FROM tag_links WHERE tag = NEW.tag;
END;
