-- Hidden photos, the archive, "On this day" and "Recently added" (docs/library.md, "Hidden, archive, memories and recently
-- added").
--
--  * assets.hidden: kept out of every list, search, map, album view and share until the owner unlocks the Hidden view with
--    their password. Enforced by the server in each query. The visible primary of a RAW + JPEG pair carries it.
--  * assets.archived: out of the timeline, the map and memories; still in albums, search and the Archive view.
--  * assets.archived_at: when it was archived (a bulk archive is undone by its batch, as a bulk trash is).
--  * assets.memory_dismissed: "not this one" in On this day.
--  * sessions.hidden_unlocked_until: the Hidden view is open for this login session until then (10 minutes after the password
--    was entered again). It lives with the session, so signing out, or any other session, never inherits it.
--  * user_prefs: the per-user switches of this release (the On this day strip, the library's sort order).
--
-- No row is rewritten and no job is queued: the new columns have constant defaults (a metadata-only change), the table is
-- new, and the only work is building indexes (a few ms per 10,000 photos).
ALTER TABLE assets
    ADD COLUMN hidden            boolean NOT NULL DEFAULT false,
    ADD COLUMN archived          boolean NOT NULL DEFAULT false,
    ADD COLUMN archived_at       timestamptz,
    ADD COLUMN memory_dismissed  boolean NOT NULL DEFAULT false;

ALTER TABLE sessions ADD COLUMN hidden_unlocked_until timestamptz;

CREATE TABLE user_prefs (
    user_id          uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    memories_enabled boolean NOT NULL DEFAULT true,
    -- The library's order: 'taken' (capture time) or 'added' (upload time).
    library_sort     text NOT NULL DEFAULT 'taken' CHECK (library_sort IN ('taken', 'added'))
);

-- The timeline, search and the day counts: visible photos that are not hidden. `archived` and the three columns of the sort
-- expression ride along (INCLUDE) so the timeline's `NOT archived` and the day counts are answered by an index-only scan
-- (no heap reads: the day counts only need the expression), while search, which keeps archived photos by default, uses the
-- same index. About 5 MB at 50,000 photos.
DROP INDEX assets_timeline_idx;
CREATE INDEX assets_timeline_idx ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC, id DESC)
    INCLUDE (archived, taken_at, file_modified_at, created_at)
    WHERE deleted_at IS NULL AND primary_id IS NULL AND NOT hidden;

-- Favorites keep archived photos but never hidden ones.
DROP INDEX assets_favorites_idx;
CREATE INDEX assets_favorites_idx
    ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC, id DESC)
    WHERE favorite AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden;

-- Recently added: the same set in upload order.
CREATE INDEX assets_added_idx ON assets (owner_id, created_at DESC, id DESC) INCLUDE (archived)
    WHERE deleted_at IS NULL AND primary_id IS NULL AND NOT hidden;

-- The Archive and Hidden views: few rows each, so a small partial index apiece.
CREATE INDEX assets_archived_idx
    ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC, id DESC)
    WHERE archived AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden;
CREATE INDEX assets_hidden_idx
    ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC, id DESC)
    WHERE hidden AND deleted_at IS NULL AND primary_id IS NULL;
