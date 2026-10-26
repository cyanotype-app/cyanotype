import { Loader2, TriangleAlert, type LucideIcon } from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import { useLayoutEffect, useState, type ReactNode } from "react";
import { PageHeader } from "../shell/PageHeader";
import { cn } from "../ui/cn";

/** Small pieces the albums, people, tags and sharing pages share: a list item that rises in, empty and failure states, a spinner. */

/** A list item that fades and rises into place, a little later the further down the list it is. */
export function Rise({ index = 0, className, children }: { index?: number; className?: string; children: ReactNode }) {
  const reduce = useReducedMotion();
  return (
    <motion.li
      className={className}
      initial={reduce ? false : { opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.26, delay: reduce ? 0 : Math.min(index, 14) * 0.022, ease: [0.2, 0.8, 0.2, 1] }}
    >
      {children}
    </motion.li>
  );
}

/** A section or block that fades in when the page has its data. */
export function Enter({ className, children }: { className?: string; children: ReactNode }) {
  const reduce = useReducedMotion();
  return (
    <motion.div
      className={className}
      initial={reduce ? false : { opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.24, ease: [0.2, 0.8, 0.2, 1] }}
    >
      {children}
    </motion.div>
  );
}

/** "Loading your albums…": quiet, centred, announced. */
export function Loading({ children, className }: { children: string; className?: string }) {
  return (
    <p role="status" className={cn("flex items-center justify-center gap-2.5 py-24 text-sm text-muted", className)}>
      <Loader2 className="size-4 animate-spin" aria-hidden="true" />
      {children}
    </p>
  );
}

/** Nothing here yet: an icon, what it is and what to do about it. */
export function Empty({
  icon: Icon,
  title,
  children,
  action,
  level = 2,
}: {
  icon: LucideIcon;
  title: string;
  children?: ReactNode;
  action?: ReactNode;
  level?: 1 | 2;
}) {
  const H = level === 1 ? "h1" : "h2";
  return (
    <div className="mx-auto flex max-w-sm flex-col items-center gap-2 px-4 py-20 text-center">
      <span className="mb-2 flex size-14 items-center justify-center rounded-full bg-surface text-muted">
        <Icon className="size-6" strokeWidth={1.6} aria-hidden="true" />
      </span>
      <H className="text-[1.0625rem] font-semibold tracking-tight">{title}</H>
      {children && <div className="text-[0.9375rem] text-muted [overflow-wrap:anywhere]">{children}</div>}
      {action && <div className="mt-3">{action}</div>}
    </div>
  );
}

/** A page that could not be loaded (or does not exist), under a header with the way back. */
export function PageProblem({ title, message, back, action }: { title: string; message: ReactNode; back: { to: string; label: string }; action?: ReactNode }) {
  return (
    <>
      <PageHeader title={title} back={back} />
      <div role="alert" className="mx-auto flex max-w-sm flex-col items-center gap-3 px-4 py-20 text-center">
        <span className="flex size-14 items-center justify-center rounded-full bg-surface text-muted">
          <TriangleAlert className="size-6" strokeWidth={1.6} aria-hidden="true" />
        </span>
        <p className="text-[0.9375rem] text-muted">{message}</p>
        {action}
      </div>
    </>
  );
}

/** A load error inside a page, with a way to try again. */
export function InlineError({ children, onRetry }: { children: ReactNode; onRetry?: () => void }) {
  return (
    <p role="alert" className="flex items-center justify-center gap-3 py-6 text-center text-sm text-danger">
      <span>{children}</span>
      {onRetry && (
        <button type="button" className="rounded-full px-3 py-1.5 font-medium text-accent hover:bg-surface" onClick={onRetry}>
          Retry
        </button>
      )}
    </p>
  );
}

/**
 * Whether the page's big title (the header's h1) is cut off with an ellipsis, so the page can show the whole of it somewhere in
 * its body: a long album title or tag name is the one thing the header cannot carry.
 */
export function useTitleCut(title: string): boolean {
  const [cut, setCut] = useState(false);
  useLayoutEffect(() => {
    const h1 = document.querySelector<HTMLElement>("header h1");
    if (!h1) return;
    const measure = () => setCut(h1.scrollWidth > h1.clientWidth + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(h1);
    return () => ro.disconnect();
  }, [title]);
  return cut;
}
