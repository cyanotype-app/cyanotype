import { useEffect, useState } from "react";
import { api, type MyUsage } from "../api";
import { blockText, limitsLine, meters, warningFor } from "../limitsText";
import { useUploads } from "./UploadProvider";
import type { UploadItem } from "./queue";

const MB = 1024 * 1024;
const size = (n: number) => (n >= MB ? `${(n / MB).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);

function label(i: UploadItem): string {
  switch (i.status) {
    case "queued":
      return "Waiting";
    case "hashing":
      return `Reading ${Math.round(i.progress * 100)}%`;
    case "checking":
      return "Checking";
    case "uploading":
      return `Uploading ${Math.round(i.progress * 100)}%`;
    case "done":
      return "Added to library";
    case "duplicate":
      return "Already in library";
    case "failed":
      return `Failed: ${i.error ?? "upload error"}`;
    case "stopped":
      return "Not started";
  }
}

/** The account's limits while a batch runs, read when it starts and again when it finishes. Null without limits. */
function useUsage(active: boolean, hasItems: boolean): MyUsage | null {
  const [usage, setUsage] = useState<MyUsage | null>(null);
  useEffect(() => {
    if (!hasItems) return;
    let live = true;
    api
      .usage()
      .then((u) => live && setUsage(u))
      .catch(() => undefined); // the tray works without it
    return () => {
      live = false;
    };
  }, [active, hasItems]);
  return usage;
}

/** Bottom-corner list of the current upload batch with an overall summary. */
export function UploadTray() {
  const { items, summary, queue } = useUploads();
  const [open, setOpen] = useState(true);
  const usage = useUsage(summary.active, items.length > 0);
  if (items.length === 0) return null;

  const parts = [`${summary.finished - summary.stopped} of ${summary.total} processed`];
  if (summary.duplicate) parts.push(`${summary.duplicate} already in library`);
  if (summary.failed) parts.push(`${summary.failed} failed`);
  if (summary.stopped) parts.push(`${summary.stopped} not started`);
  const m = usage ? meters(usage) : null;
  const line = usage ? limitsLine(usage) : null;
  const warnings = [warningFor(m?.quota ?? null, "storage"), warningFor(m?.daily ?? null, "daily upload limit")].filter(
    (w): w is string => w !== null,
  );

  return (
    <section className="tray" aria-label="Uploads">
      <header className="tray-head">
        <div>
          <strong>{summary.active ? "Uploading" : summary.blocked ? "Uploads stopped" : "Uploads finished"}</strong>
          <div className="small muted" role="status">
            {parts.join(" · ")}
          </div>
        </div>
        <div className="tray-actions">
          <button type="button" className="btn btn-quiet" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
            {open ? "Hide" : "Show"}
          </button>
          {!summary.active && (
            <button type="button" className="btn btn-quiet" onClick={() => queue.clearFinished()}>
              Clear
            </button>
          )}
        </div>
      </header>
      {summary.blocked && (
        <div className="tray-block" role="alert">
          <strong>{blockText(summary.blocked)}</strong>
          <div className="small">{summary.blocked.message}</div>
          <button type="button" className="btn btn-quiet" onClick={() => queue.retryBlocked()}>
            Try again
          </button>
        </div>
      )}
      {line && (
        <p className="tray-limits small muted" aria-label="Your limits">
          {line}
        </p>
      )}
      {!summary.blocked &&
        warnings.map((w) => (
          <p key={w} className="tray-warning small" role="status">
            {w}
          </p>
        ))}
      <progress className="tray-bar" max={1} value={summary.progress} aria-label="Overall upload progress" />
      {open && (
        <ul className="tray-list">
          {items.map((i) => (
            <li key={i.id} className={`tray-item tray-${i.status}`}>
              <span className="tray-name" title={i.file.name}>
                {i.file.name}
              </span>
              <span className="small muted">{size(i.file.size)}</span>
              <span className="small tray-status">{label(i)}</span>
              {i.status === "failed" && !i.block && (
                <button type="button" className="btn btn-quiet" onClick={() => queue.retry(i.id)}>
                  Retry
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
