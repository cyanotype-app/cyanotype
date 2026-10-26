import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Check, ChevronDown, ChevronUp, CircleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type MyUsage } from "../api";
import { blockText, limitsLine, meters, warningFor } from "../limitsText";
import { cn } from "../ui/cn";
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

/** Bottom-corner list of the current upload batch with an overall summary: a card above the tab bar on a phone. */
export function UploadTray() {
  const { items, summary, queue } = useUploads();
  const [open, setOpen] = useState(true);
  const reduce = useReducedMotion();
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
  const small = "h-8 rounded-full px-3 text-[0.8125rem] font-medium text-muted hover:bg-surface hover:text-fg";

  return (
    <motion.section
      aria-label="Uploads"
      initial={reduce ? false : { opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.22, ease: [0.2, 0.8, 0.2, 1] }}
      className="fixed right-3 left-3 z-30 flex max-h-[60dvh] flex-col overflow-hidden rounded-card border border-line bg-card shadow-float md:right-4 md:left-auto md:w-[24rem]"
      style={{ bottom: "calc(max(var(--tabbar-h, 0px), var(--selbar-h, 0px)) + 4.5rem)" }}
    >
      <header className="flex items-center gap-2 px-4 pt-3 pb-2">
        <div className="min-w-0 flex-1">
          <strong className="text-[0.9375rem] font-semibold">
            {summary.active ? "Uploading" : summary.blocked ? "Uploads stopped" : "Uploads finished"}
          </strong>
          <div className="truncate text-[0.8125rem] text-muted" role="status">
            {parts.join(" · ")}
          </div>
        </div>
        <button type="button" className={small} onClick={() => setOpen((o) => !o)} aria-expanded={open}>
          {open ? <ChevronDown className="inline size-4" aria-hidden="true" /> : <ChevronUp className="inline size-4" aria-hidden="true" />}
          <span className="sr-only">{open ? "Hide" : "Show"}</span>
        </button>
        {!summary.active && (
          <button type="button" className={small} onClick={() => queue.clearFinished()}>
            Clear
          </button>
        )}
      </header>
      <div className="px-4 pb-3">
        <progress
          className="block h-1 w-full appearance-none overflow-hidden rounded-full bg-surface [&::-moz-progress-bar]:bg-accent [&::-webkit-progress-bar]:bg-surface [&::-webkit-progress-value]:bg-accent [&::-webkit-progress-value]:transition-[width] [&::-webkit-progress-value]:duration-300"
          max={1}
          value={summary.progress}
          aria-label="Overall upload progress"
        />
      </div>
      {summary.blocked && (
        <div className="mx-4 mb-3 rounded-xl bg-surface p-3 text-sm" role="alert">
          <strong>{blockText(summary.blocked)}</strong>
          <div className="mt-0.5 text-muted">{summary.blocked.message}</div>
          <button type="button" className="mt-2 h-8 rounded-full bg-accent px-3 text-[0.8125rem] font-medium text-on-accent" onClick={() => queue.retryBlocked()}>
            Try again
          </button>
        </div>
      )}
      {line && (
        <p className="mx-4 mb-2 text-[0.8125rem] text-muted" aria-label="Your limits">
          {line}
        </p>
      )}
      {!summary.blocked &&
        warnings.map((w) => (
          <p key={w} className="mx-4 mb-2 text-[0.8125rem] text-danger" role="status">
            {w}
          </p>
        ))}
      <AnimatePresence initial={false}>
        {open && (
          <motion.ul
            key="list"
            initial={reduce ? false : { height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2 }}
            className="m-0 min-h-0 list-none overflow-y-auto border-t border-line p-0"
          >
            {items.map((i) => (
              <li key={i.id} className="flex items-center gap-2 border-b border-line px-4 py-2 text-[0.8125rem] last:border-b-0">
                <span className="min-w-0 flex-1 truncate" title={i.file.name}>
                  {i.file.name}
                </span>
                <span className="shrink-0 text-muted">{size(i.file.size)}</span>
                <span
                  className={cn(
                    "inline-flex min-w-0 max-w-[45%] items-center gap-1 truncate text-muted",
                    i.status === "failed" && "text-danger",
                    i.status === "done" && "text-accent",
                  )}
                >
                  {i.status === "done" && <Check className="size-3.5" aria-hidden="true" />}
                  {i.status === "failed" && <CircleAlert className="size-3.5" aria-hidden="true" />}
                  {label(i)}
                </span>
                {i.status === "failed" && !i.block && (
                  <button type="button" className={small} onClick={() => queue.retry(i.id)}>
                    Retry
                  </button>
                )}
              </li>
            ))}
          </motion.ul>
        )}
      </AnimatePresence>
    </motion.section>
  );
}
