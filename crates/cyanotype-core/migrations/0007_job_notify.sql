-- Wake workers the moment a job becomes runnable (LISTEN/NOTIFY), instead of waiting for their poll.
--
-- A trigger rather than a call in each enqueue path, so every way a job becomes queued is covered: upload complete,
-- the metadata -> pairing hand-off, reindex --rethumb, the retry endpoint, a failed job going back to 'queued' for a
-- delayed retry, and ad-hoc SQL. The payload is the job kind; Postgres collapses identical notifications within a
-- transaction, so a bulk insert of 50,000 rows delivers a handful of messages.
--
-- Delayed retries (run_after in the future) notify too; the worker then finds nothing due and sleeps until the
-- earliest run_after. NOTIFY is transactional: listeners hear it only after the enqueuing transaction commits.
CREATE FUNCTION jobs_notify() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('jobs_queued', NEW.kind);
    RETURN NULL;
END
$$;

CREATE TRIGGER jobs_notify_insert AFTER INSERT ON jobs
    FOR EACH ROW WHEN (NEW.state = 'queued') EXECUTE FUNCTION jobs_notify();

-- Only transitions into 'queued' (retry, requeue) or a changed run_after on a queued job; the worker's own
-- claim/complete updates must not wake every worker.
CREATE TRIGGER jobs_notify_update AFTER UPDATE ON jobs
    FOR EACH ROW WHEN (NEW.state = 'queued' AND (OLD.state IS DISTINCT FROM 'queued' OR OLD.run_after IS DISTINCT FROM NEW.run_after))
    EXECUTE FUNCTION jobs_notify();

-- Unfinished jobs by state: the queue gauges, the per-user processing status and the retry endpoint.
CREATE INDEX jobs_open_idx ON jobs (state, kind) WHERE state <> 'done';
