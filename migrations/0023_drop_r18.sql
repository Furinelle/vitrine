-- The gallery has no R18 content; ingest and admin no longer read or write this flag.
ALTER TABLE works DROP COLUMN is_r18;
