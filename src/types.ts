export interface Env {
  DB: D1Database;
  MEDIA: R2Bucket;
  ASSETS: Fetcher;
  INGEST_TOKEN: string;
  PUBLIC_BASE_URL?: string;
}

export interface IngestMeta {
  source: string;
  source_id: string;
  source_url?: string;
  title?: string;
  author_name?: string;
  author_url?: string;
  tags?: string[];
  origin?: string;
}

export interface WorkRow {
  id: string;
  source: string;
  source_id: string;
  source_url: string;
  title: string;
  author_name: string;
  author_url: string;
  page_count: number;
  origin: string;
  created_at: string;
  cover_key?: string | null;
  tags?: string;
}
