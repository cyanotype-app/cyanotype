-- Favorites: a per-photo flag. For a RAW + JPEG pair the visible primary carries it (the hidden RAW never does).
-- Adding a column with a constant default is a metadata-only change on PostgreSQL 11+, so this is instant.
ALTER TABLE assets ADD COLUMN favorite boolean NOT NULL DEFAULT false;

-- GET /assets?favorite=true and GET /assets/timeline?favorite=true: the same keyset order as assets_timeline_idx but
-- only over favorites, so the Favorites view is an index range scan however large the library is.
CREATE INDEX assets_favorites_idx
    ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC, id DESC)
    WHERE favorite AND deleted_at IS NULL AND primary_id IS NULL;
