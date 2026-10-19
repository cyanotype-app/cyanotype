-- Admin portal, part 2: what each account stores (docs/admin.md, "Usage").
--
-- `user_usage` is one row per account with the sizes and counts the admin accounts page and the user's Storage page
-- show. It is kept current by triggers on `assets`, not by calls in each code path: a photo reaches `assets` through
-- upload, reindex, the CLI, pairing and bulk SQL, and leaves it through trash, restore, purge and cascade, so a trigger
-- covers them all (like `jobs_notify` in 0007 and the burst trigger in 0014). The worker's nightly reconcile
-- (cyanotype_core::usage) recomputes every row from `assets` and reports and repairs any drift.
--
--  * originals_bytes / file_count: files that are not in the trash (every file, so the hidden RAW half of a pair counts).
--  * trash_bytes / trash_count: files in the trash, until they are purged (the row is deleted then).
--  * photo_count: what the library lists (not trashed, not the hidden half of a RAW + JPEG pair).
--  * raw_count: RAW files that are not in the trash (includes the hidden halves).
--  * derived_bytes: thumbnails, previews and share copies, MEASURED where the worker (or the reconcile's catch-up) has
--    recorded them; derived_files is how many files that covers, so the pages can say "measured for n of m".
--
-- Creating the table scans `assets` once to fill it (one grouped pass); no other row is rewritten and no job is queued.
CREATE TABLE user_usage (
    user_id         uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    originals_bytes bigint NOT NULL DEFAULT 0,
    trash_bytes     bigint NOT NULL DEFAULT 0,
    derived_bytes   bigint NOT NULL DEFAULT 0,
    photo_count     bigint NOT NULL DEFAULT 0,
    raw_count       bigint NOT NULL DEFAULT 0,
    file_count      bigint NOT NULL DEFAULT 0,
    trash_count     bigint NOT NULL DEFAULT 0,
    derived_files   bigint NOT NULL DEFAULT 0,
    updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Sizes of the derived objects, set by the worker when it writes them (NULL: not measured yet).
ALTER TABLE assets ADD COLUMN derived_bytes bigint, ADD COLUMN share_copy_bytes bigint;

-- Mirrors `pairing::RAW_EXTS` (a test keeps the two in step).
CREATE FUNCTION asset_is_raw(ext text) RETURNS boolean LANGUAGE sql IMMUTABLE AS $$
    SELECT ext = ANY (ARRAY[
        'cr2','cr3','crw','nef','nrw','arw','srf','sr2','raf','orf','rw2','rwl','pef','ptx','dng','srw',
        '3fr','fff','iiq','erf','kdc','dcr','mef','mos','mrw','raw','rwz','x3f','gpr','cap','eip'])
$$;

-- Statement-level triggers over transition tables: one upsert per owner per statement. (A row-level trigger updating the same
-- `user_usage` row once per file made a 50,000-photo insert twenty times slower, as every update lengthens the row's version
-- chain inside the transaction.) A statement that changes nothing that counts costs a pass over its rows and no write.
CREATE FUNCTION assets_usage_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO user_usage (user_id, originals_bytes, trash_bytes, derived_bytes, photo_count, raw_count, file_count,
                            trash_count, derived_files)
    SELECT d.owner_id,
           coalesce(sum(CASE WHEN d.deleted_at IS NULL THEN d.s * d.size_bytes ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NOT NULL THEN d.s * d.size_bytes ELSE 0 END), 0),
           coalesce(sum(d.s * (coalesce(d.derived_bytes, 0) + coalesce(d.share_copy_bytes, 0))), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL AND d.primary_id IS NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL AND asset_is_raw(d.file_ext) THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NOT NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.derived_bytes IS NOT NULL THEN d.s ELSE 0 END), 0)
    FROM (SELECT *, 1 AS s FROM new_rows) d
    -- An owner that no longer exists (an account being deleted: its files go by cascade) gets no row.
    WHERE EXISTS (SELECT 1 FROM users WHERE id = d.owner_id)
    GROUP BY d.owner_id
    ON CONFLICT (user_id) DO UPDATE SET
        originals_bytes = user_usage.originals_bytes + EXCLUDED.originals_bytes,
        trash_bytes     = user_usage.trash_bytes + EXCLUDED.trash_bytes,
        derived_bytes   = user_usage.derived_bytes + EXCLUDED.derived_bytes,
        photo_count     = user_usage.photo_count + EXCLUDED.photo_count,
        raw_count       = user_usage.raw_count + EXCLUDED.raw_count,
        file_count      = user_usage.file_count + EXCLUDED.file_count,
        trash_count     = user_usage.trash_count + EXCLUDED.trash_count,
        derived_files   = user_usage.derived_files + EXCLUDED.derived_files,
        updated_at      = now();
    RETURN NULL;
END
$$;

CREATE FUNCTION assets_usage_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO user_usage (user_id, originals_bytes, trash_bytes, derived_bytes, photo_count, raw_count, file_count,
                            trash_count, derived_files)
    SELECT d.owner_id,
           coalesce(sum(CASE WHEN d.deleted_at IS NULL THEN d.s * d.size_bytes ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NOT NULL THEN d.s * d.size_bytes ELSE 0 END), 0),
           coalesce(sum(d.s * (coalesce(d.derived_bytes, 0) + coalesce(d.share_copy_bytes, 0))), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL AND d.primary_id IS NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL AND asset_is_raw(d.file_ext) THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NOT NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.derived_bytes IS NOT NULL THEN d.s ELSE 0 END), 0)
    FROM (SELECT *, -1 AS s FROM old_rows) d
    -- An owner that no longer exists (an account being deleted: its files go by cascade) gets no row.
    WHERE EXISTS (SELECT 1 FROM users WHERE id = d.owner_id)
    GROUP BY d.owner_id
    ON CONFLICT (user_id) DO UPDATE SET
        originals_bytes = user_usage.originals_bytes + EXCLUDED.originals_bytes,
        trash_bytes     = user_usage.trash_bytes + EXCLUDED.trash_bytes,
        derived_bytes   = user_usage.derived_bytes + EXCLUDED.derived_bytes,
        photo_count     = user_usage.photo_count + EXCLUDED.photo_count,
        raw_count       = user_usage.raw_count + EXCLUDED.raw_count,
        file_count      = user_usage.file_count + EXCLUDED.file_count,
        trash_count     = user_usage.trash_count + EXCLUDED.trash_count,
        derived_files   = user_usage.derived_files + EXCLUDED.derived_files,
        updated_at      = now();
    RETURN NULL;
END
$$;

-- An update subtracts the old versions of the rows and adds the new ones; what did not change nets to zero.
CREATE FUNCTION assets_usage_update() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO user_usage (user_id, originals_bytes, trash_bytes, derived_bytes, photo_count, raw_count, file_count,
                            trash_count, derived_files)
    SELECT d.owner_id,
           coalesce(sum(CASE WHEN d.deleted_at IS NULL THEN d.s * d.size_bytes ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NOT NULL THEN d.s * d.size_bytes ELSE 0 END), 0),
           coalesce(sum(d.s * (coalesce(d.derived_bytes, 0) + coalesce(d.share_copy_bytes, 0))), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL AND d.primary_id IS NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL AND asset_is_raw(d.file_ext) THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.deleted_at IS NOT NULL THEN d.s ELSE 0 END), 0),
           coalesce(sum(CASE WHEN d.derived_bytes IS NOT NULL THEN d.s ELSE 0 END), 0)
    FROM (SELECT *, -1 AS s FROM old_rows UNION ALL SELECT *, 1 AS s FROM new_rows) d
    -- An owner that no longer exists (an account being deleted: its files go by cascade) gets no row.
    WHERE EXISTS (SELECT 1 FROM users WHERE id = d.owner_id)
    GROUP BY d.owner_id
    ON CONFLICT (user_id) DO UPDATE SET
        originals_bytes = user_usage.originals_bytes + EXCLUDED.originals_bytes,
        trash_bytes     = user_usage.trash_bytes + EXCLUDED.trash_bytes,
        derived_bytes   = user_usage.derived_bytes + EXCLUDED.derived_bytes,
        photo_count     = user_usage.photo_count + EXCLUDED.photo_count,
        raw_count       = user_usage.raw_count + EXCLUDED.raw_count,
        file_count      = user_usage.file_count + EXCLUDED.file_count,
        trash_count     = user_usage.trash_count + EXCLUDED.trash_count,
        derived_files   = user_usage.derived_files + EXCLUDED.derived_files,
        updated_at      = now();
    RETURN NULL;
END
$$;

CREATE TRIGGER assets_usage_insert AFTER INSERT ON assets
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION assets_usage_insert();
CREATE TRIGGER assets_usage_delete AFTER DELETE ON assets
    REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION assets_usage_delete();
CREATE TRIGGER assets_usage_update AFTER UPDATE ON assets
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION assets_usage_update();

-- The initial fill, from the truth (the triggers keep it after this).
INSERT INTO user_usage (user_id, originals_bytes, trash_bytes, derived_bytes, photo_count, raw_count, file_count,
                        trash_count, derived_files)
SELECT owner_id,
       coalesce(sum(size_bytes) FILTER (WHERE deleted_at IS NULL), 0),
       coalesce(sum(size_bytes) FILTER (WHERE deleted_at IS NOT NULL), 0),
       0,
       count(*) FILTER (WHERE deleted_at IS NULL AND primary_id IS NULL),
       count(*) FILTER (WHERE deleted_at IS NULL AND asset_is_raw(file_ext)),
       count(*) FILTER (WHERE deleted_at IS NULL),
       count(*) FILTER (WHERE deleted_at IS NOT NULL),
       0
FROM assets GROUP BY owner_id;

-- What each account uploaded and when, for "in the last 24 hours / 30 days" (and, later, the daily cap). One row per
-- completed upload, written by the assets service; a reindex or an import by SQL is not an upload. Pruned at 90 days.
CREATE TABLE upload_log (
    id       bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    at       timestamptz NOT NULL DEFAULT now(),
    bytes    bigint NOT NULL CHECK (bytes >= 0)
);
CREATE INDEX upload_log_owner_idx ON upload_log (owner_id, at DESC);
CREATE INDEX upload_log_at_idx ON upload_log (at);

-- Small facts of the server itself (when the reconcile last ran, what it found), as JSON.
CREATE TABLE server_state (
    key        text PRIMARY KEY,
    value      jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Background work an admin asked for, run by the worker (the bucket size check now; account deletion later). One row per
-- request, with the result or error kept for the page. A task `running` for over two hours is taken to be abandoned.
CREATE TABLE admin_tasks (
    id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind         text NOT NULL,
    state        text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'done', 'failed')),
    requested_by uuid REFERENCES users(id) ON DELETE SET NULL,
    params       jsonb NOT NULL DEFAULT '{}',
    result       jsonb,
    error        text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    started_at   timestamptz,
    finished_at  timestamptz
);
CREATE INDEX admin_tasks_open_idx ON admin_tasks (id) WHERE state IN ('queued', 'running');
-- One bucket check at a time.
CREATE UNIQUE INDEX admin_tasks_one_bucket_check ON admin_tasks (kind)
    WHERE kind = 'bucket_check' AND state IN ('queued', 'running');
