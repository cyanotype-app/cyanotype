import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";

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
  const reduce = useReducedMotion();

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
    <div
      className="pointer-events-none fixed inset-x-0 bottom-[calc(var(--tabbar-h)+0.75rem)] z-40 flex justify-center px-4 md:bottom-6"
      role="status"
      aria-live="polite"
    >
      <AnimatePresence>
        {spec && (
          <motion.div
            key={spec.message}
            initial={reduce ? false : { opacity: 0, y: 12, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ duration: reduce ? 0 : 0.2, ease: [0.2, 0.8, 0.2, 1] }}
            className={cn(
              "pointer-events-auto flex max-w-md items-center gap-2 rounded-full py-1.5 pr-1.5 pl-4 text-sm shadow-float",
              spec.tone === "error" ? "bg-danger text-on-danger" : "bg-fg text-bg",
            )}
          >
            <span className="min-w-0 py-1.5 break-words">{spec.message}</span>
            {spec.action && (
              <Button
                size="sm"
                variant="ghost"
                className="text-inherit hover:bg-white/15"
                onClick={() => {
                  const run = spec.action!.onClick;
                  hide();
                  run();
                }}
              >
                {spec.action.label}
              </Button>
            )}
            <Button size="icon-sm" variant="ghost" className="text-inherit hover:bg-white/15" aria-label="Dismiss" onClick={hide}>
              <X />
            </Button>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
  return { toast, show, hide };
}
