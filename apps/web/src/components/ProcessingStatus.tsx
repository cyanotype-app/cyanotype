import { useCallback, useEffect, useRef, useState } from "react";
import { api as defaultApi, type Api, type Processing } from "../api";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
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
    <section aria-label="Background processing" className="mx-4 mb-3 rounded-card bg-surface px-4 py-2.5 md:mx-8">
      <div className="flex min-h-7 items-center gap-3">
        {label && (
          <p className="min-w-0 flex-1 truncate text-sm" role="status">
            {label}
          </p>
        )}
        {status.failed > 0 && (
          <Button
            size="sm"
            variant="quiet"
            className={cn("ml-auto text-danger", !label && "flex-1 justify-start")}
            aria-expanded={open}
            aria-controls="processing-panel"
            onClick={() => setOpen(!open)}
          >
            {status.failed.toLocaleString("en-US")} failed
          </Button>
        )}
      </div>
      {label && (
        <div
          className="mt-2 h-1 overflow-hidden rounded-full bg-line"
          role="progressbar"
          aria-label="Processing progress"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent ?? undefined}
        >
          <span
            className={cn("block h-full rounded-full bg-accent transition-[width] duration-500 ease-soft", percent === null && "w-1/3 animate-pulse")}
            style={percent === null ? undefined : { width: `${percent}%` }}
          />
        </div>
      )}
      {open && status.failed > 0 && (
        <div className="mt-3 flex flex-col gap-3 border-t border-line pt-3" id="processing-panel">
          <div className="flex items-center justify-between gap-3">
            <strong className="text-sm font-semibold">Failed</strong>
            <Button size="sm" variant="primary" disabled={busy} onClick={() => void retry()}>
              Retry failed
            </Button>
          </div>
          {error && (
            <p role="alert" className="text-sm text-danger">
              {error}
            </p>
          )}
          <ul className="flex flex-col gap-2.5">
            {status.failures.map((f) => (
              <li key={f.job_id} className="flex flex-col gap-0.5">
                <span className="truncate text-sm">{f.original_name}</span>
                <span className="text-[0.8125rem] text-muted">
                  {kindLabel(f.kind)}, {f.attempts} {f.attempts === 1 ? "attempt" : "attempts"}
                </span>
                {f.last_error && <span className="text-[0.8125rem] break-words text-muted">{f.last_error}</span>}
              </li>
            ))}
          </ul>
          {status.failed > status.failures.length && (
            <p className="text-[0.8125rem] text-muted">Showing the latest {status.failures.length} of {status.failed.toLocaleString("en-US")}.</p>
          )}
        </div>
      )}
    </section>
  );
}
