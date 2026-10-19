import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Health } from "../../api";
import { bytesLabel } from "../../adminText";
import { backupStatus, duration, failedKinds, memoryLine, queueTotals } from "../../healthText";
import { ago, countLabel } from "../../storageText";

const REFRESH_MS = 15_000;

/** Admin > Health: the queue, failed jobs (with a retry), the worker's memory, the last backup, sizes and the version. */
export function HealthPage() {
  const [h, setH] = useState<Health | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const live = useRef(true);

  const load = useCallback(async () => {
    try {
      const next = await api.health();
      if (live.current) {
        setH(next);
        setError(null);
      }
    } catch (e) {
      if (live.current) setError(e instanceof Error ? e.message : "Could not load the health card.");
    }
  }, []);
  useEffect(() => {
    live.current = true;
    void load();
    const t = setInterval(() => void load(), REFRESH_MS);
    return () => {
      live.current = false;
      clearInterval(t);
    };
  }, [load]);

  async function retry(kind?: string) {
    setBusy(true);
    setNote(null);
    try {
      const r = await api.retryAllFailedJobs(kind);
      setNote(r.requeued === 0 ? "Nothing to retry." : `${countLabel(r.requeued, "job")} put back in the queue.`);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not retry the jobs.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section aria-labelledby="health-title">
      <h2 id="health-title">Health</h2>
      <p className="muted">What the server knows about itself. It refreshes every 15 seconds.</p>
      <div role="alert" className="form-error">
        {error}
      </div>
      {h === null ? (
        !error && (
          <p className="muted" role="status">
            Loading…
          </p>
        )
      ) : (
        <>
          <Queue h={h} busy={busy} onRetry={(k) => void retry(k)} note={note} />
          <Workers h={h} />
          <Backup h={h} />
          <Sizes h={h} />
          <Version h={h} />
        </>
      )}
    </section>
  );
}

function Queue({ h, busy, onRetry, note }: { h: Health; busy: boolean; onRetry: (kind?: string) => void; note: string | null }) {
  const t = queueTotals(h.queue);
  const kinds = failedKinds(h.queue);
  return (
    <section className="health-card" aria-labelledby="queue-title">
      <h3 id="queue-title">Job queue</h3>
      <p className="small">
        {t.queued === 0 && t.running === 0 && t.failed === 0
          ? "Nothing waiting."
          : `${countLabel(t.queued, "job")} waiting, ${t.running} running, ${t.failed} failed.`}
        {h.stalled_seconds > 60 && (
          <span className="health-bad"> No job has finished for {duration(h.stalled_seconds)} while some are waiting.</span>
        )}
      </p>
      {h.queue.length > 0 && (
        <ul className="health-queue">
          {h.queue.map((k) => (
            <li key={k.kind}>
              <strong>{k.kind}</strong>
              <span className="muted small">
                {k.queued.toLocaleString("en-US")} waiting · {k.running} running ·{" "}
                <span className={k.failed ? "health-bad" : ""}>{k.failed.toLocaleString("en-US")} failed</span>
                {k.queued > 0 && ` · oldest waiting ${duration(k.oldest_queued_age_secs)}`}
              </span>
            </li>
          ))}
        </ul>
      )}
      {t.failed > 0 && (
        <>
          <div className="invite-actions">
            <button type="button" className="btn btn-quiet" disabled={busy} onClick={() => onRetry()}>
              Retry all failed jobs
            </button>
            {kinds.length > 1 &&
              kinds.map((k) => (
                <button key={k.kind} type="button" className="btn btn-quiet" disabled={busy} onClick={() => onRetry(k.kind)}>
                  Retry {k.kind} ({k.failed.toLocaleString("en-US")})
                </button>
              ))}
          </div>
          <ul className="failed-jobs small" aria-label="Failed jobs">
            {h.failed_jobs.map((j) => (
              <li key={j.id}>
                <strong>{j.kind}</strong> after {j.attempts} attempts{j.finished_at ? `, ${ago(j.finished_at)}` : ""}
                {j.last_error && <div className="muted">{j.last_error}</div>}
              </li>
            ))}
          </ul>
          {t.failed > h.failed_jobs.length && <p className="muted small">Showing the latest {h.failed_jobs.length}.</p>}
        </>
      )}
      <p className="small" role="status">
        {note}
      </p>
    </section>
  );
}

function Workers({ h }: { h: Health }) {
  return (
    <section className="health-card" aria-labelledby="workers-title">
      <h3 id="workers-title">Worker</h3>
      {h.workers.length === 0 ? (
        <p className="muted small">No worker has reported yet. It reports once a minute after it starts.</p>
      ) : (
        <ul className="health-queue">
          {h.workers.map((w) => (
            <li key={w.host}>
              <strong>{w.host}</strong>
              <span className="small">
                {w.reporting ? (
                  memoryLine(w)
                ) : (
                  <span className="health-bad">not reporting (last heard {ago(w.at)})</span>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Backup({ h }: { h: Health }) {
  const s = backupStatus(h.backup);
  return (
    <section className="health-card" aria-labelledby="backup-title">
      <h3 id="backup-title">Backup</h3>
      <p className={`small ${s.level === "ok" ? "" : "health-bad"}`} role="status">
        {s.text}
      </p>
    </section>
  );
}

function Sizes({ h }: { h: Health }) {
  const s = h.storage;
  return (
    <section className="health-card" aria-labelledby="sizes-title">
      <h3 id="sizes-title">Sizes</h3>
      <dl className="stat-grid">
        <div className="stat">
          <dt>Database</dt>
          <dd>{bytesLabel(s.database_bytes)}</dd>
        </div>
        <div className="stat">
          <dt>Photos stored</dt>
          <dd>{bytesLabel(s.stored_bytes)}</dd>
        </div>
        <div className="stat">
          <dt>Thumbnails</dt>
          <dd>{bytesLabel(s.derived_bytes)}</dd>
        </div>
        <div className="stat">
          <dt>Bucket</dt>
          <dd>
            {s.bucket_check && s.bucket_check.current_bytes != null ? (
              <>
                {bytesLabel(s.bucket_check.current_bytes)}
                {s.bucket_check.noncurrent_bytes ? ` + ${bytesLabel(s.bucket_check.noncurrent_bytes)} old versions` : ""}
                <span className="muted small"> checked {ago(s.bucket_check.at)}</span>
              </>
            ) : (
              <span className="muted small">Run the bucket check on the Accounts page.</span>
            )}
          </dd>
        </div>
      </dl>
    </section>
  );
}

function Version({ h }: { h: Health }) {
  return (
    <section className="health-card" aria-labelledby="version-title">
      <h3 id="version-title">Version</h3>
      <p className="small">
        Assets service <code>{h.version.assets}</code>
        {h.version.workers.map((w) => (
          <span key={w.host}>
            {" "}
            · worker <code>{w.version ?? "unknown"}</code>
          </span>
        ))}
      </p>
    </section>
  );
}
