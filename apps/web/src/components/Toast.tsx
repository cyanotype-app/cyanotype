import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "../ui/cn";

export interface ToastSpec {
  message: string;
  /** An optional single action, e.g. "Undo". */
  action?: { label: string; onClick: () => void };
  tone?: "info" | "error";
  /** Milliseconds before it hides itself (default 8000; errors 10000). */
  ms?: number;
}

/**
 * One toast at a time, announced politely: a dark pill that rises from above the tab bar (or the selection bar, which
 * publishes its height as `--selbar-h`). `show` replaces the current toast.
 */
export function useToast(): { toast: ReactNode; show: (t: ToastSpec) => void; hide: () => void } {
  const [spec, setSpec] = useState<(ToastSpec & { n: number }) | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const count = useRef(0);
  const reduce = useReducedMotion();

  const hide = useCallback(() => {
    clearTimeout(timer.current);
    setSpec(null);
  }, []);
  const show = useCallback((t: ToastSpec) => {
    clearTimeout(timer.current);
    setSpec({ ...t, n: ++count.current });
    timer.current = setTimeout(() => setSpec(null), t.ms ?? (t.tone === "error" ? 10000 : 8000));
  }, []);
  useEffect(() => () => clearTimeout(timer.current), []);

  const toast = (
    <div
      className="pointer-events-none fixed inset-x-0 z-40 flex justify-center px-3"
      style={{ bottom: "calc(max(var(--tabbar-h, 0px), var(--selbar-h, 0px)) + 0.75rem)" }}
      role="status"
      aria-live="polite"
    >
      <AnimatePresence>
        {spec && (
          <motion.div
            key={spec.n}
            initial={reduce ? { opacity: 0 } : { opacity: 0, y: 16, scale: 0.97 }}
            animate={reduce ? { opacity: 1 } : { opacity: 1, y: 0, scale: 1 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: 8, scale: 0.98 }}
            transition={{ duration: reduce ? 0 : 0.22, ease: [0.2, 0.8, 0.2, 1] }}
            data-tone={spec.tone === "error" ? "error" : "info"}
            className={cn(
              "pointer-events-auto flex max-w-[min(28rem,100%)] items-center gap-2 rounded-full py-1.5 pr-1.5 pl-4 text-[0.9375rem] shadow-float ring-1",
              spec.tone === "error" ? "bg-danger text-on-danger ring-white/10" : "bg-[#101820] text-white ring-white/10",
            )}
          >
            <span className="min-w-0 py-1 leading-snug">{spec.message}</span>
            {spec.action && (
              <button
                type="button"
                className="h-8 shrink-0 rounded-full bg-white/15 px-3 text-sm font-semibold text-white hover:bg-white/25 active:scale-[0.97]"
                onClick={() => {
                  const run = spec.action!.onClick;
                  hide();
                  run();
                }}
              >
                {spec.action.label}
              </button>
            )}
            <button
              type="button"
              className="grid size-8 shrink-0 grid-cols-1 place-items-center rounded-full text-white/70 hover:bg-white/15 hover:text-white"
              aria-label="Dismiss"
              onClick={hide}
            >
              <X className="size-4" aria-hidden="true" />
            </button>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
  return { toast, show, hide };
}
