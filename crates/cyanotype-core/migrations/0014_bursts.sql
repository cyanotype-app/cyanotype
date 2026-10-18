-- Bursts: rapid-fire shots of one moment, grouped so search can show them as one result (docs/library.md, "Bursts").
--
-- A burst is a chain of photos of one owner and one camera, taken at most ~2 s apart, each near-identical to the one
-- before it by embedding cosine (crates/cyanotype-core/src/bursts.rs). It is computed by the worker's `burst` jobs and
-- stored here, so a search reads it instead of computing it:
--  * `burst_id`: the id of the group's earliest photo, on every member of a group of two or more; NULL for a photo that
--    is on its own. Only visible photos (not trashed, not the hidden half of a RAW + JPEG pair) are ever in a group.
--  * `burst_version`: the grouping rules the row was last computed with (`bursts::VERSION`); NULL means "to be
--    computed". Raising the constant makes the worker's sweep recompute every photo, a model change is covered by the
--    `burst` job each new vector queues.
-- No rows are touched here: the existing library is grouped by the worker's sweep (a job per photo that has an embedding,
-- at backfill priority), so this migration is instant at any size.
ALTER TABLE assets ADD COLUMN burst_id uuid, ADD COLUMN burst_version integer;

-- A group's members (search expands a burst; "More like this" leaves out the source's own).
CREATE INDEX assets_burst_idx ON assets (owner_id, burst_id) WHERE burst_id IS NOT NULL;

-- The chain around a capture time: the owner's visible photos of one camera by capture time.
CREATE INDEX assets_burst_window_idx ON assets (owner_id, camera_label, taken_at)
    WHERE deleted_at IS NULL AND primary_id IS NULL AND taken_at IS NOT NULL AND camera_label IS NOT NULL;

-- Each queued `burst` job is a fresh row (jobs are unique per kind, photo and version, and a regroup must run again
-- whenever the neighbourhood changes, also while an earlier one is running): its version comes from this sequence.
CREATE SEQUENCE burst_job_seq;

-- A photo that appears in, or leaves, what is listed (trash, restore, a RAW paired to it or unpaired again) changes the
-- groups around it. A trigger rather than a call in each code path, like `jobs_notify` (0007): it covers the trash
-- endpoints, bulk scopes, pairing, and ad-hoc SQL alike. It marks the photo as "to be computed" and queues a regroup of
-- its neighbourhood (the job reads the photo's camera and time even when it is hidden now, which is how a removed photo's
-- neighbours get regrouped). Purging needs nothing: the photo was trashed first.
-- (AFTER, not BEFORE: `camera_label` is a generated column, which a BEFORE trigger cannot read yet.)
CREATE FUNCTION burst_changed() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.taken_at IS NOT NULL AND NEW.camera_label IS NOT NULL THEN
        UPDATE assets SET burst_version = NULL WHERE id = NEW.id AND burst_version IS NOT NULL;
        IF NOT EXISTS (SELECT 1 FROM jobs WHERE kind = 'burst' AND asset_id = NEW.id AND state = 'queued') THEN
            INSERT INTO jobs (kind, asset_id, version, priority) VALUES ('burst', NEW.id, nextval('burst_job_seq'), 5);
        END IF;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER assets_burst_changed AFTER UPDATE OF deleted_at, primary_id ON assets
    FOR EACH ROW WHEN (OLD.deleted_at IS DISTINCT FROM NEW.deleted_at OR OLD.primary_id IS DISTINCT FROM NEW.primary_id)
    EXECUTE FUNCTION burst_changed();
