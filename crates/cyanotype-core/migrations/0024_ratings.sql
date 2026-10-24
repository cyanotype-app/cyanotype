-- Star ratings, 0 to 5 (docs/library.md, "Star ratings").
--
--  * assets.rating: 0 is "not rated". A RAW + JPEG pair carries it on its visible primary, as the favorite flag is.
--  * assets.rating_source: who set it. NULL = nobody (rating 0), 'file' = read from the photo's own metadata (EXIF Rating or
--    XMP xmp:Rating) when it was imported, 'user' = chosen in the app (including a deliberate 0). The worker's metadata job
--    only writes a rating while the source is NULL or 'file', so a rating the owner chose is never overwritten, and a photo
--    whose file says 4 stars but whose owner cleared it stays cleared.
--
-- No row is rewritten and no job is queued: both columns have constant defaults (a metadata-only change). The partial index
-- holds only rated photos (usually a small share), in timeline order, so "at least N stars" is an index scan.
ALTER TABLE assets
    ADD COLUMN rating        smallint NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
    ADD COLUMN rating_source text CHECK (rating_source IN ('file', 'user'));

CREATE INDEX assets_rating_idx
    ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC, id DESC) INCLUDE (rating)
    WHERE rating > 0 AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden;
