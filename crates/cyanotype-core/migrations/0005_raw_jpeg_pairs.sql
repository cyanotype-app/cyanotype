-- RAW + JPEG pairs: a camera's RAW and its JPEG/HEIC rendering of the same shot show up as one photo.
--
-- Both files stay separate assets (own row, own S3 original, own derived objects, own sha256 dedupe). The RAW is the
-- secondary: it points at its primary through assets.primary_id and is hidden from the library, albums, counts and
-- the trash listing. NULL = a normal, visible photo. Everything that acts on a photo (trash, restore, purge,
-- albums) goes through the primary and takes its RAW along.
--
-- Invariants kept by the application, under an advisory lock per (owner, filename stem):
--   * the target of primary_id is itself visible (its primary_id is NULL), and has at most one RAW (unique index);
--   * both files of a pair share deleted_at / purging_at.
-- ON DELETE SET NULL: purging deletes both rows in the same sweep; if the primary goes first the RAW is already
-- marked purging, so it stays hidden by the trash/purging filters and is deleted next.
ALTER TABLE assets ADD COLUMN primary_id uuid REFERENCES assets(id) ON DELETE SET NULL;
ALTER TABLE assets ADD CONSTRAINT assets_primary_not_self CHECK (primary_id IS NULL OR primary_id <> id);
CREATE UNIQUE INDEX assets_primary_id_idx ON assets (primary_id) WHERE primary_id IS NOT NULL;

-- Pairs the user separated by hand ("Unpair"); the matcher never re-pairs them. Cascades with either asset.
CREATE TABLE asset_pair_rejections (
    raw_id     uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    primary_id uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (raw_id, primary_id)
);
CREATE INDEX asset_pair_rejections_primary_idx ON asset_pair_rejections (primary_id);

-- Filename without directory, extension and case: the matcher's candidate lookup (the Rust side applies the same
-- rule and re-checks every candidate, so this only narrows the search).
CREATE FUNCTION asset_stem(name text) RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE AS
$$ SELECT lower(regexp_replace(regexp_replace(name, '^.*[/\\]', ''), '\.[^.]*$', '')) $$;
CREATE INDEX assets_owner_stem_idx ON assets (owner_id, asset_stem(original_name));

-- The timeline lists visible photos only; keep its index exactly that set.
DROP INDEX assets_timeline_idx;
CREATE INDEX assets_timeline_idx ON assets (owner_id, (coalesce(taken_at, file_modified_at, created_at)) DESC, id DESC)
    WHERE deleted_at IS NULL AND primary_id IS NULL;

-- Backfill: one 'pair' job per existing live photo whose metadata is done (new uploads pair when their own metadata
-- job finishes). The worker drains them in the background; each is a few indexed lookups and a no-op for files that
-- are neither RAW nor JPEG/HEIC. Re-running is harmless (ON CONFLICT, and pairing is idempotent).
INSERT INTO jobs (kind, asset_id, version)
SELECT 'pair', id, 1 FROM assets WHERE deleted_at IS NULL AND purging_at IS NULL AND width IS NOT NULL
ON CONFLICT DO NOTHING;
