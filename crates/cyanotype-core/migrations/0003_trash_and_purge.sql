-- Trash and permanent deletion.
--
-- deleted_at  = in the trash (restorable until purged).
-- purging_at  = queued for permanent deletion: hidden from the trash, no longer restorable, and its S3 objects
--               are being removed (retried by the assets janitor until the row itself is deleted). A row is
--               only ever purging while deleted_at is also set.
ALTER TABLE assets ADD COLUMN purging_at timestamptz;
ALTER TABLE assets ADD CONSTRAINT assets_purging_implies_deleted CHECK (purging_at IS NULL OR deleted_at IS NOT NULL);

-- GET /assets/trash: newest deletions first, keyset-paged on (deleted_at, id).
CREATE INDEX assets_trash_idx ON assets (owner_id, deleted_at DESC, id DESC)
    WHERE deleted_at IS NOT NULL AND purging_at IS NULL;
-- Janitor: trashed rows past retention (across owners), and rows waiting for their S3 objects to be removed.
CREATE INDEX assets_trash_age_idx ON assets (deleted_at) WHERE deleted_at IS NOT NULL AND purging_at IS NULL;
CREATE INDEX assets_purging_idx ON assets (purging_at) WHERE purging_at IS NOT NULL;
-- Dedupe lookups (check / complete) by content hash already use the (owner_id, sha256) unique index.
