-- Tags (docs/library.md, "Tags"): global per-user labels on photos.
--
--  * tags: one row per name and owner. Two names are the same tag when they are equal ignoring case (the unique index), and the
--    spelling kept is the first one used. Names are flat; "/" is allowed in them for a hierarchy later, nothing reads it as one.
--    No comma and no control character (the CHECK mirrors `cyanotype_core::tags::normalize`): a comma separates a list of tags
--    in an address.
--  * asset_tags: which photos have which tags. `imported` marks a tag that came from the photo's own metadata (IPTC Keywords,
--    XMP dc:subject) and not from the owner; the owner adding the same tag makes it theirs. Both sides cascade: deleting a photo
--    for good, or a tag, removes the links. The visible photo of a RAW + JPEG pair carries the tags (pairing moves a RAW's).
--  * assets.tags_imported_at: when the photo's keywords were read. They are read once, so a tag the owner removed from a photo
--    does not return when the metadata job runs again; `reindex` sets it for a photo it restores from a manifest that knows tags.
--
-- No row is rewritten and no job is queued: new tables, and a nullable column without a default (metadata-only). Existing photos
-- have no tags until their metadata job runs again (a rebuild, or `cyanotype reindex --backfill-metadata`); nothing here queues it.
CREATE TABLE tags (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       text NOT NULL CHECK (name = btrim(name) AND char_length(name) BETWEEN 1 AND 100 AND name !~ '[,[:cntrl:]]'),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX tags_owner_name_idx ON tags (owner_id, lower(name));

CREATE TABLE asset_tags (
    asset_id uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    tag_id   uuid NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    imported boolean NOT NULL DEFAULT false,
    added_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (asset_id, tag_id)
);
-- "Which photos have this tag" (the tag filter, the tag page, counts) and the cascade from a deleted tag.
CREATE INDEX asset_tags_tag_idx ON asset_tags (tag_id, asset_id);

ALTER TABLE assets ADD COLUMN tags_imported_at timestamptz;
