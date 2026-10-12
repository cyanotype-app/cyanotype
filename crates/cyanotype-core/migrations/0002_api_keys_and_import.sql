-- API keys: a non-secret display prefix so the list can tell keys apart (the key itself is only ever
-- stored as key_hash).
ALTER TABLE api_keys ADD COLUMN prefix text NOT NULL DEFAULT '';
CREATE INDEX api_keys_user_idx ON api_keys (user_id);

-- Bulk import: the original file's mtime, a timeline fallback for files without EXIF taken_at.
ALTER TABLE assets  ADD COLUMN file_modified_at timestamptz;
ALTER TABLE uploads ADD COLUMN file_modified_at timestamptz;

-- The timeline orders by coalesce(taken_at, file_modified_at, created_at); rebuild its index to match.
DROP INDEX assets_timeline_idx;
CREATE INDEX assets_timeline_idx ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC)
    WHERE deleted_at IS NULL;

-- Find a pending upload to resume (check) and stale ones to abort (janitor).
CREATE INDEX uploads_owner_sha_idx ON uploads (owner_id, sha256);
CREATE INDEX uploads_created_idx ON uploads (created_at);
