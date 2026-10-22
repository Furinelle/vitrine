-- Hand-picked image sets drawn from tagged works, e.g. every image that really shows Furina.
-- Only 'keep' images are published to the set's consumers; images with no row await review.
CREATE TABLE image_curation (
  collection TEXT NOT NULL,
  r2_key TEXT NOT NULL,
  verdict TEXT NOT NULL CHECK (verdict IN ('keep', 'exclude')),
  decided_at TEXT NOT NULL,
  PRIMARY KEY (collection, r2_key)
);

CREATE INDEX idx_image_curation_verdict ON image_curation(collection, verdict);
