import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { api, type Health } from "../../api";
import { bytesLabel } from "../../adminText";
import { backupStatus, duration, failedKinds, memoryLine, queueTotals } from "../../healthText";
import { ago, countLabel } from "../../storageText";
import { Card, FormError, Loading, Mono } from "../../settings/kit";
import { Button } from "../../ui/Button";
import { cn } from "../../ui/cn";
import { IntegrityCard } from "./IntegrityCard";

const REFRESH_MS = 15_000;

const bad = "text-danger";

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
    <div className="[overflow-wrap:anywhere]">
      <p className="text-sm text-muted">What the server knows about itself. It refreshes every 15 seconds.</p>
      <div className="mt-3">
        <FormError>{error}</FormError>
      </div>
      {h === null ? (
        !error && <Loading />
      ) : (
        <div className="mt-4 flex flex-col gap-4">
          <Queue h={h} busy={busy} onRetry={(k) => void retry(k)} note={note} />
          <IntegrityCard />
          <Workers h={h} />
          <Backup h={h} />
          <Sizes h={h} />
          <Version h={h} />
        </div>
      )}
    </div>
  );
}

function Queue({ h, busy, onRetry, note }: { h: Health; busy: boolean; onRetry: (kind?: string) => void; note: string | null }) {
  const t = queueTotals(h.queue);
  const kinds = failedKinds(h.queue);
  return (
    <Card id="queue-title" title="Job queue">
      <p>
        {t.queued === 0 && t.running === 0 && t.failed === 0
          ? "Nothing waiting."
          : `${countLabel(t.queued, "job")} waiting, ${t.running} running, ${t.failed} failed.`}
        {h.stalled_seconds > 60 && <span className={bad}> No job has finished for {duration(h.stalled_seconds)} while some are waiting.</span>}
      </p>
      {h.queue.length > 0 && (
        <ul className="divide-y divide-line">
          {h.queue.map((k) => (
            <li key={k.kind} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-0.5 py-2 first:pt-0 last:pb-0">
              <strong className="font-medium">{k.kind}</strong>
              <span className="text-[0.8125rem] text-muted">
                {k.queued.toLocaleString("en-US")} waiting · {k.running} running ·{" "}
                <span className={k.failed ? bad : ""}>{k.failed.toLocaleString("en-US")} failed</span>
                {k.queued > 0 && ` · oldest waiting ${duration(k.oldest_queued_age_secs)}`}
              </span>
            </li>
          ))}
        </ul>
      )}
      {t.failed > 0 && (
        <>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" disabled={busy} onClick={() => onRetry()}>
              Retry all failed jobs
            </Button>
            {kinds.length > 1 &&
              kinds.map((k) => (
                <Button key={k.kind} size="sm" disabled={busy} onClick={() => onRetry(k.kind)}>
                  Retry {k.kind} ({k.failed.toLocaleString("en-US")})
                </Button>
              ))}
          </div>
          <ul className="flex flex-col gap-2 rounded-xl bg-surface p-3" aria-label="Failed jobs">
            {h.failed_jobs.map((j) => (
              <li key={j.id}>
                <strong className="font-medium">{j.kind}</strong> after {j.attempts} attempts{j.finished_at ? `, ${ago(j.finished_at)}` : ""}
                {j.last_error && <div className="break-words text-muted">{j.last_error}</div>}
              </li>
            ))}
          </ul>
          {t.failed > h.failed_jobs.length && <p className="text-[0.8125rem] text-muted">Showing the latest {h.failed_jobs.length}.</p>}
        </>
      )}
      <p role="status" className="empty:hidden">
        {note}
      </p>
    </Card>
  );
}

function Workers({ h }: { h: Health }) {
  return (
    <Card id="workers-title" title="Worker">
      {h.workers.length === 0 ? (
        <p className="text-muted">No worker has reported yet. It reports once a minute after it starts.</p>
      ) : (
        <ul className="divide-y divide-line">
          {h.workers.map((w) => (
            <li key={w.host} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-0.5 py-2 first:pt-0 last:pb-0">
              <strong className="font-medium">{w.host}</strong>
              <span>{w.reporting ? memoryLine(w) : <span className={bad}>not reporting (last heard {ago(w.at)})</span>}</span>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

function Backup({ h }: { h: Health }) {
  const s = backupStatus(h.backup);
  return (
    <Card id="backup-title" title="Backup">
      <p className={cn(s.level !== "ok" && bad)} role="status">
        {s.text}
      </p>
    </Card>
  );
}

function Sizes({ h }: { h: Health }) {
  const s = h.storage;
  const stat = (label: string, value: ReactNode) => (
    <div className="min-w-0">
      <dt className="text-xs text-muted">{label}</dt>
      <dd className="mt-0.5 break-words text-[0.9375rem]">{value}</dd>
    </div>
  );
  return (
    <Card id="sizes-title" title="Sizes">
      <dl className="grid grid-cols-2 gap-x-4 gap-y-3 min-[560px]:grid-cols-4">
        {stat("Database", bytesLabel(s.database_bytes))}
        {stat("Photos stored", bytesLabel(s.stored_bytes))}
        {stat("Thumbnails", bytesLabel(s.derived_bytes))}
        {stat(
          "Bucket",
          s.bucket_check && s.bucket_check.current_bytes != null ? (
            <>
              {bytesLabel(s.bucket_check.current_bytes)}
              {s.bucket_check.noncurrent_bytes ? ` + ${bytesLabel(s.bucket_check.noncurrent_bytes)} old versions` : ""}
              <span className="text-[0.8125rem] text-muted"> checked {ago(s.bucket_check.at)}</span>
            </>
          ) : (
            <span className="text-[0.8125rem] text-muted">Run the bucket check on the Accounts page.</span>
          ),
        )}
      </dl>
    </Card>
  );
}

function Version({ h }: { h: Health }) {
  return (
    <Card id="version-title" title="Version">
      <p>
        Assets service <Mono>{h.version.assets}</Mono>
        {h.version.workers.map((w) => (
          <span key={w.host}>
            {" "}
            · worker <Mono>{w.version ?? "unknown"}</Mono>
          </span>
        ))}
      </p>
    </Card>
  );
}
