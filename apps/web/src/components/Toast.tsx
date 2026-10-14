import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

export interface ToastSpec {
  message: string;
  /** An optional single action, e.g. "Undo". */
  action?: { label: string; onClick: () => void };
  tone?: "info" | "error";
  /** Milliseconds before it hides itself (default 8000; errors 10000). */
  ms?: number;
}

/** One toast at a time, announced politely. `show` replaces the current toast. */
export function useToast(): { toast: ReactNode; show: (t: ToastSpec) => void; hide: () => void } {
  const [spec, setSpec] = useState<ToastSpec | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const hide = useCallback(() => {
    clearTimeout(timer.current);
    setSpec(null);
  }, []);
  const show = useCallback((t: ToastSpec) => {
    clearTimeout(timer.current);
    setSpec(t);
    timer.current = setTimeout(() => setSpec(null), t.ms ?? (t.tone === "error" ? 10000 : 8000));
  }, []);
  useEffect(() => () => clearTimeout(timer.current), []);

  const toast = (
    <div className="toast-region" role="status" aria-live="polite">
      {spec && (
        <div className={`toast${spec.tone === "error" ? " toast-error" : ""}`}>
          <span>{spec.message}</span>
          {spec.action && (
            <button
              type="button"
              className="btn btn-glass"
              onClick={() => {
                const run = spec.action!.onClick;
                hide();
                run();
              }}
            >
              {spec.action.label}
            </button>
          )}
          <button type="button" className="toast-close" aria-label="Dismiss" onClick={hide}>
            ×
          </button>
        </div>
      )}
    </div>
  );
  return { toast, show, hide };
}
