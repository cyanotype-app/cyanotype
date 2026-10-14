-- Albums: named, owner-scoped collections of photos. An album references photos, it never owns them: deleting an
-- album deletes only its membership rows, and a purged photo drops out of every album through the cascade.
-- Trashing a photo keeps its membership (restoring puts it back); album queries simply skip trashed photos.
CREATE TABLE albums (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id       uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title          text NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 200),
    description    text CHECK (description IS NULL OR char_length(description) <= 5000),
    -- Explicit cover; when null (or the photo is trashed / removed) the album shows its most recent photo.
    cover_asset_id uuid REFERENCES assets(id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
-- GET /assets/albums: most recently updated first.
CREATE INDEX albums_owner_updated_idx ON albums (owner_id, updated_at DESC, id DESC);

CREATE TABLE album_assets (
    album_id uuid NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    asset_id uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    added_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (album_id, asset_id)
);
-- The primary key serves "the photos of an album" (joined to assets for timeline order); this one serves
-- "which albums contain this asset" and the cascade when an asset is purged.
CREATE INDEX album_assets_asset_idx ON album_assets (asset_id, album_id);
