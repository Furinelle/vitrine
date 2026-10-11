-- Oracle computes a consistent snapshot; any relevant write invalidates its generation.
CREATE TABLE similar_generation (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  generation INTEGER NOT NULL CHECK (generation > 0)
);
INSERT INTO similar_generation (id, generation) VALUES (1, 1);

CREATE TRIGGER similar_works_insert AFTER INSERT ON works BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_works_update AFTER UPDATE ON works BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_works_delete AFTER DELETE ON works BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_images_insert AFTER INSERT ON images BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_images_update AFTER UPDATE ON images BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_images_delete AFTER DELETE ON images BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_fingerprints_insert AFTER INSERT ON image_fingerprints BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_fingerprints_update AFTER UPDATE ON image_fingerprints BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_fingerprints_delete AFTER DELETE ON image_fingerprints BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_clip_insert AFTER INSERT ON image_clip BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_clip_update AFTER UPDATE ON image_clip BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER similar_clip_delete AFTER DELETE ON image_clip BEGIN
  UPDATE similar_generation SET generation=generation+1 WHERE id=1;
END;
