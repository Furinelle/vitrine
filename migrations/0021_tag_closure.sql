-- The tag library becomes the single source of implied tags: a tag implies its
-- parent (character -> work, skin -> character, AI tool -> AI生成) and, for a
-- pairing, its members (a JSON array of tag names). tag_closure follows those
-- edges transitively; tag_links stays the materialised copy the 0017 trigger
-- reads on every work_tags insert, refreshed from the view after library edits.
ALTER TABLE tag_items ADD COLUMN members TEXT;

CREATE VIEW tag_closure AS
WITH RECURSIVE direct(tag, also) AS (
  SELECT name, parent FROM tag_items WHERE parent IS NOT NULL
  UNION
  SELECT i.name, m.value FROM tag_items i, json_each(i.members) m WHERE i.members IS NOT NULL
), closure(tag, also) AS (
  SELECT tag, also FROM direct
  UNION
  SELECT c.tag, d.also FROM closure c JOIN direct d ON d.tag = c.also
)
SELECT tag, also FROM closure WHERE tag <> also;
