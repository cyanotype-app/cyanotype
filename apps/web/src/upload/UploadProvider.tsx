import { createContext, useContext, useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import { api } from "../api";
import { hashFile } from "./hash";
import { UploadQueue, summarize, type QueueDeps, type Summary, type UploadItem } from "./queue";

interface UploadContextValue {
  queue: UploadQueue;
  items: readonly UploadItem[];
  summary: Summary;
}

const UploadContext = createContext<UploadContextValue | null>(null);

/**
 * Owns the upload queue. Mounted in the authenticated shell, so uploads keep running while the user moves
 * between pages of the SPA, and a tab close is guarded while anything is in flight.
 */
export function UploadProvider({ children, deps }: { children: ReactNode; deps?: QueueDeps }) {
  const [queue] = useState(() => new UploadQueue(deps ?? { api, hash: hashFile }));
  const items = useSyncExternalStore(queue.subscribe, queue.getSnapshot);
  const summary = useMemo(() => summarize(items), [items]);

  useEffect(() => {
    if (!summary.active) return;
    const guard = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = ""; // legacy browsers need a value to show the prompt
    };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [summary.active]);

  const value = useMemo(() => ({ queue, items, summary }), [queue, items, summary]);
  return <UploadContext.Provider value={value}>{children}</UploadContext.Provider>;
}

export function useUploads(): UploadContextValue {
  const v = useContext(UploadContext);
  if (!v) throw new Error("useUploads must be used inside <UploadProvider>");
  return v;
}
