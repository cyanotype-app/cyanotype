import { useCallback, useEffect, useRef, useState } from "react";
import { api as defaultApi, type Api, type Processing } from "../api";
import { kindLabel, nextPollMs, processingLabel, processingPercent } from "../processingText";

/**
 * Polls GET /assets/processing: every 3 s while photos are being processed, then backing off (10 s, 20 s ... 60 s)
 * while idle, and not at all while the tab is hidden. A failed poll keeps the last status and tries again later.
 */
export function useProcessing(api: Pick<Api, "processing"> = defaultApi) {
  const [status, setStatus] = useState<Processing | null>(null);
  const reload = useRef<() => void>(() => {});
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let idle = 0;
    const tick = async () => {
      clearTimeout(timer);
      let active = false;
      if (!document.hidden) {
        try {
          const p = await api.processing();
          if (stopped) return;
          setStatus(p);
          active = p.active;
          idle = p.active ? 0 : idle + 1;
        } catch {
          idle += 1;
        }
      }
      if (!stopped) timer = setTimeout(() => void tick(), nextPollMs(active, Math.max(0, idle - 1)));
    };
    reload.current = () => void tick();
    const onVisible = () => {
      if (!document.hidden) void tick();
    };
    document.addEventListener("visibilitychange", onVisible);
    void tick();
    return () => {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [api]);
  return { status, reload: useCallback(() => reload.current(), []) };
}

/** A slim banner in the library header: progress while photos are processed, and failures with a retry. Hidden when idle. */
export function ProcessingStatus({ api = defaultApi }: { api?: Pick<Api, "processing" | "retryFailedJobs"> }) {
  const { status, reload } = useProcessing(api);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!status || (!status.active && status.failed === 0)) return null;

  const label = processingLabel(status);
  const percent = processingPercent(status);
  const retry = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.retryFailedJobs();
      reload();
    } catch {
      setError("Couldn’t retry. Try again in a moment.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="processing" aria-label="Background processing">
      <div className="processing-row">
        {label && (
          <p className="processing-label" role="status">
            {label}
          </p>
        )}
        {status.failed > 0 && (
          <button
            type="button"
            className="btn btn-quiet processing-failed"
            aria-expanded={open}
            aria-controls="processing-panel"
            onClick={() => setOpen(!open)}
          >
            {status.failed.toLocaleString("en-US")} failed
          </button>
        )}
      </div>
      {label && (
        <div
          className="processing-bar"
          role="progressbar"
          aria-label="Processing progress"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent ?? undefined}
        >
          <span className={percent === null ? "indeterminate" : undefined} style={percent === null ? undefined : { width: `${percent}%` }} />
        </div>
      )}
      {open && status.failed > 0 && (
        <div className="processing-panel" id="processing-panel">
          <div className="processing-panel-head">
            <strong>Failed</strong>
            <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void retry()}>
              Retry failed
            </button>
          </div>
          {error && <p role="alert" className="small">{error}</p>}
          <ul className="processing-failures">
            {status.failures.map((f) => (
              <li key={f.job_id}>
                <span className="processing-file">{f.original_name}</span>
                <span className="muted small">
                  {kindLabel(f.kind)}, {f.attempts} {f.attempts === 1 ? "attempt" : "attempts"}
                </span>
                {f.last_error && <span className="muted small processing-error">{f.last_error}</span>}
              </li>
            ))}
          </ul>
          {status.failed > status.failures.length && (
            <p className="muted small">Showing the latest {status.failures.length} of {status.failed.toLocaleString("en-US")}.</p>
          )}
        </div>
      )}
    </section>
  );
}
