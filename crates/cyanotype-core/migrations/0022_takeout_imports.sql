-- Google Takeout import (docs/import.md, "Moving from Google Photos").
--
-- What a Takeout sidecar says about a photo (its capture time, location, caption, favourite flag, and the album folder it was
-- in) is sent with the upload's `POST /assets/check`, keyed by the photo's content hash, and held here until the photo has
-- been processed: the worker's `takeout` job applies it as an *edit* over the file's own EXIF (cyanotype_core::edits, so it
-- is revertible and survives a rebuild from the bucket like any edit), a favourite and album memberships. Held on the server
-- so closing the browser tab, or stopping the command line, after a photo was checked cannot lose it, and keyed by hash so it
-- works whether the photo is uploaded now or already in the library (a re-run applies the metadata to photos that exist).
--
--  * `rev` counts payload changes; a `takeout` job is queued per revision, so a re-run with different metadata applies again
--    and one with the same metadata does nothing. `applied_rev = rev` means done.
--  * Rows nothing ever applied (the upload never happened) are deleted by the worker after 30 days.
-- One empty table; no row rewritten, no job queued. The applied results are ordinary edits, favourites and albums, which the
-- bucket manifests already carry; this table is a queue and is not part of any manifest.
CREATE TABLE takeout_imports (
    owner_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sha256      bytea NOT NULL,
    payload     jsonb NOT NULL,
    rev         integer NOT NULL DEFAULT 1,
    applied_rev integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    applied_at  timestamptz,
    PRIMARY KEY (owner_id, sha256)
);
-- The worker's sweep looks at what is not applied yet.
CREATE INDEX takeout_imports_pending_idx ON takeout_imports (updated_at) WHERE applied_rev < rev;
