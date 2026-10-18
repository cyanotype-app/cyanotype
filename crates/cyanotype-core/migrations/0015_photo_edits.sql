-- Photo edits: a corrected date and time (with its time zone), a corrected or added location, and a caption
-- (docs/library.md, "Editing a photo").
--
-- `taken_at`, `latitude` and `longitude` stay the EFFECTIVE values: everything that reads them (the timeline order, the
-- map, the date filters, bursts, pairing, geocoding) keeps working unchanged. An edit overwrites them and keeps what the
-- file itself said next to them (`*_exif`) so "Revert to original" can put it back; the worker's metadata job writes the
-- file's values into `*_exif` instead of the effective columns while a photo is edited. The original in the bucket is never
-- touched. The edits are also written to the bucket (`edits/{user}/{xx}/{sha256}.json`), which is what `cyanotype reindex`
-- restores them from.
--
-- No rows are touched: new columns with constant defaults, so this is instant at any size.
ALTER TABLE assets
    ADD COLUMN date_edited      boolean NOT NULL DEFAULT false,
    ADD COLUMN taken_at_exif    timestamptz,
    -- The zone the edited time is shown in, seconds east of UTC; NULL = unknown (the time is then read as UTC).
    ADD COLUMN taken_offset_edit integer,
    ADD COLUMN location_edited  boolean NOT NULL DEFAULT false,
    ADD COLUMN latitude_exif    double precision,
    ADD COLUMN longitude_exif   double precision,
    ADD COLUMN caption          text CHECK (caption IS NULL OR (caption <> '' AND char_length(caption) <= 2000)),
    -- When the edits last changed (the `updated_at` of the object in the bucket): reindex applies an edit object only
    -- when it is newer than this.
    ADD COLUMN edited_at        timestamptz;

-- Caption search looks only at the (few) photos that have one.
CREATE INDEX assets_caption_idx ON assets (owner_id) WHERE caption IS NOT NULL AND deleted_at IS NULL AND primary_id IS NULL;
