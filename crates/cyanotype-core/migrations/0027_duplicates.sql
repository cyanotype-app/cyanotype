-- Near-duplicate review (docs/library.md, "Near-duplicates"): photos that are the same picture twice (a resized or re-encoded copy, a
-- shot repeated seconds later), found by the worker from the image embeddings and written here for the owner to review.
--
--  * duplicate_groups / duplicate_members: the groups of the last scan of an owner (two or more photos each). A scan replaces them;
--    they are derived (nothing in the bucket, nothing to restore: the next scan finds them again). Hidden photos are grouped like
--    any other and left out when a group is READ by a session that has not unlocked the Hidden view.
--  * duplicate_dismissals: "Not duplicates", permanent: every pair of the photos of a dismissed group, `a < b`. A scan never groups
--    such a pair again. They follow their photos (the foreign keys cascade when a photo is purged).
--  * duplicate_scans: one row per owner, the request and the state of a scan. `requested_at` set = a scan is wanted (since then);
--    `manual` = the owner asked (it runs at once), otherwise it follows imports and waits for a quiet period; started/finished and
--    what it found are for the page.
--
-- No row is rewritten and no job is queued: new tables only.
CREATE TABLE duplicate_groups (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    -- The newest photo's time, to show recent groups first.
    sort_time  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX duplicate_groups_owner_idx ON duplicate_groups (owner_id, sort_time DESC, id DESC);

CREATE TABLE duplicate_members (
    group_id   uuid NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
    asset_id   uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, asset_id)
);
CREATE INDEX duplicate_members_asset_idx ON duplicate_members (asset_id);

CREATE TABLE duplicate_dismissals (
    owner_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    a          uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    b          uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (a, b),
    CHECK (a < b)
);
CREATE INDEX duplicate_dismissals_owner_idx ON duplicate_dismissals (owner_id);

CREATE TABLE duplicate_scans (
    owner_id     uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    requested_at timestamptz,
    manual       boolean NOT NULL DEFAULT false,
    started_at   timestamptz,
    finished_at  timestamptz,
    groups       integer,
    photos       integer,
    error        text
);
CREATE INDEX duplicate_scans_due_idx ON duplicate_scans (requested_at) WHERE requested_at IS NOT NULL;
