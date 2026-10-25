-- Storage integrity check (docs/admin.md, "Storage integrity"): the nightly scan that compares the database with the
-- bucket. `integrity_runs` is one row per run (what it counted); `integrity_findings` is what it found, one row per
-- problem until it is fixed, acknowledged or gone. Both tables are small: one finding per damaged photo or stray object,
-- one run per night.
--
-- `assets.verified_at` is when the worker last re-read the original and found its sha256 to be the stored one (the rolling
-- checksum sample, oldest first). A nullable column without a default: nothing is rewritten, and the index below is
-- built over the rows once. New uploads start NULL, which puts them first in line.
ALTER TABLE assets ADD COLUMN verified_at timestamptz;
CREATE INDEX assets_verify_idx ON assets (verified_at NULLS FIRST, created_at, id) WHERE purging_at IS NULL;

CREATE TABLE integrity_runs (
    id                   bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- 'schedule' (the nightly run) or 'manual' (Admin > Health > Run check now).
    trigger              text NOT NULL CHECK (trigger IN ('schedule', 'manual')),
    requested_by         uuid REFERENCES users(id) ON DELETE SET NULL,
    state                text NOT NULL DEFAULT 'running' CHECK (state IN ('running', 'done', 'failed')),
    started_at           timestamptz NOT NULL DEFAULT now(),
    finished_at          timestamptz,
    error                text,
    -- What the run looked at.
    objects_listed       bigint NOT NULL DEFAULT 0,
    assets_checked       bigint NOT NULL DEFAULT 0,
    assets_verified      bigint NOT NULL DEFAULT 0,
    bytes_hashed         bigint NOT NULL DEFAULT 0,
    -- What it found (still open when the run ended) and what it repaired by itself.
    missing_originals    integer NOT NULL DEFAULT 0,
    untracked_objects    integer NOT NULL DEFAULT 0,
    checksum_mismatches  integer NOT NULL DEFAULT 0,
    derived_requeued     integer NOT NULL DEFAULT 0,
    findings_resolved    integer NOT NULL DEFAULT 0
);
CREATE INDEX integrity_runs_started_idx ON integrity_runs (started_at DESC);

CREATE TABLE integrity_findings (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind           text NOT NULL CHECK (kind IN ('missing_original', 'untracked_object', 'checksum_mismatch')),
    -- 'open' until someone acts or a later run sees the problem gone ('resolved'); 'acknowledged' = an admin marked it
    -- lost / left it: still true, no longer counted, not raised again.
    state          text NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'acknowledged', 'resolved')),
    -- No foreign key on the owner: an object can sit under an account that does not exist.
    owner_id       uuid,
    asset_id       uuid REFERENCES assets(id) ON DELETE CASCADE,
    key            text NOT NULL,
    detail         jsonb NOT NULL DEFAULT '{}',
    resolution     text,
    first_run      bigint REFERENCES integrity_runs(id) ON DELETE SET NULL,
    last_run       bigint REFERENCES integrity_runs(id) ON DELETE SET NULL,
    first_seen_at  timestamptz NOT NULL DEFAULT now(),
    last_seen_at   timestamptz NOT NULL DEFAULT now(),
    resolved_at    timestamptz,
    resolved_by    uuid REFERENCES users(id) ON DELETE SET NULL
);
-- One live finding per problem: a run that sees it again updates it.
CREATE UNIQUE INDEX integrity_findings_live_idx ON integrity_findings (kind, key) WHERE state <> 'resolved';
CREATE INDEX integrity_findings_list_idx ON integrity_findings (state, kind, id DESC);
CREATE INDEX integrity_findings_asset_idx ON integrity_findings (asset_id) WHERE asset_id IS NOT NULL;

-- One integrity check queued or running at a time (admin_tasks queues it for the worker, like the bucket check).
CREATE UNIQUE INDEX admin_tasks_one_integrity_check ON admin_tasks (kind)
    WHERE kind = 'integrity_check' AND state IN ('queued', 'running');
