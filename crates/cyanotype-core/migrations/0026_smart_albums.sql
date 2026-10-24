-- Smart albums (docs/library.md, "Smart albums"): an album defined by saved rules instead of members.
--
--  * albums.rules: NULL for an ordinary album; for a smart one the validated rules as JSON
--    ({"match": "all"|"any", "archived": "include"|"exclude", "rules": [...]}). Nothing is materialised: the photos of the album are
--    whatever the rules match when the album is looked at (the same SQL as the search filters), so no `album_assets` rows exist for it.
--
-- No row is rewritten and no job is queued: a nullable column without a default (metadata-only).
ALTER TABLE albums ADD COLUMN rules jsonb CHECK (rules IS NULL OR jsonb_typeof(rules) = 'object');
