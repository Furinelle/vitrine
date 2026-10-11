-- L2-normalized CLIP ViT-B/32 embeddings (512 f32 LE, hex-encoded). Computed on Oracle.
CREATE TABLE image_clip (
  r2_key TEXT PRIMARY KEY,
  dim INTEGER NOT NULL,
  embedding TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
