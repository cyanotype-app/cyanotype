-- Share links: a public, unauthenticated view of one album or of a fixed set of photos.
--
-- `token` is the secret in the URL (/s/<token>), 128+ random bits, URL-safe. It is stored in plaintext (unique
-- index) rather than hashed so the owner can copy a link again later; docs/library.md explains the trade-off.
-- Revoking deletes the row, so the link stops working on the very next request (nothing is cached).
CREATE TABLE shares (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id          uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token             text NOT NULL UNIQUE CHECK (char_length(token) >= 22),
    -- 'album' follows the album live (photos added later appear, trashed ones disappear); 'selection' is the fixed
    -- set in share_assets (trashed or purged photos disappear). Deleting the album revokes its links.
    kind              text NOT NULL CHECK (kind IN ('album', 'selection')),
    album_id          uuid REFERENCES albums(id) ON DELETE CASCADE,
    title             text CHECK (title IS NULL OR char_length(btrim(title)) BETWEEN 1 AND 200),
    -- argon2id PHC string; null = no password.
    password_hash     text,
    -- Offer downloads at all.
    allow_download    boolean NOT NULL DEFAULT true,
    -- Downloads serve the untouched originals (which carry location) instead of a location-free copy.
    include_originals boolean NOT NULL DEFAULT false,
    expires_at        timestamptz,
    view_count        bigint NOT NULL DEFAULT 0,
    last_viewed_at    timestamptz,
    -- Wrong passwords since the last right one, and the lockout they caused.
    pw_failures       integer NOT NULL DEFAULT 0,
    pw_locked_until   timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    CHECK ((kind = 'album') = (album_id IS NOT NULL))
);
CREATE INDEX shares_owner_idx ON shares (owner_id, created_at DESC, id DESC);
CREATE INDEX shares_album_idx ON shares (album_id) WHERE album_id IS NOT NULL;

-- The photos of a 'selection' share (the visible primary of a RAW + JPEG pair).
CREATE TABLE share_assets (
    share_id uuid NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    asset_id uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    PRIMARY KEY (share_id, asset_id)
);
CREATE INDEX share_assets_asset_idx ON share_assets (asset_id);

-- A visitor who gave the right password: the cookie holds a random value, this keeps its sha256. Scoped to one
-- share (the cookie path is that share's API prefix), short-lived, and gone with the share or on a password change.
CREATE TABLE share_unlocks (
    token_hash bytea PRIMARY KEY,
    share_id   uuid NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL
);
CREATE INDEX share_unlocks_share_idx ON share_unlocks (share_id);

-- Set by the worker once derived/{owner}/{asset}/share.jpg (the JPEG with all metadata but orientation removed)
-- exists. Downloads through a share offer a JPEG only once this is set.
ALTER TABLE assets ADD COLUMN share_copy_at timestamptz;
