import { motion, useReducedMotion } from "motion/react";
import { Link } from "react-router-dom";
import { useId, type ReactNode } from "react";
import { cn } from "../ui/cn";

/** A titled block of the Collections page, with an optional link at the title's right ("See all"). */
export function Section({
  title,
  to,
  linkLabel = "See all",
  linkName,
  children,
  className,
}: {
  title: string;
  to?: string;
  linkLabel?: string;
  /** The link's accessible name when "See all" alone would not say what it opens. */
  linkName?: string;
  children: ReactNode;
  className?: string;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className={cn("min-w-0", className)}>
      <div className="mb-3 flex items-baseline justify-between gap-4 px-4 md:px-8">
        <h2 id={id} className="text-[1.0625rem] font-semibold tracking-tight">
          {title}
        </h2>
        {to && (
          <Link
            to={to}
            aria-label={linkName}
            className="-my-2 rounded-full px-2 py-2 text-[0.9375rem] text-accent no-underline hover:bg-surface"
          >
            {linkLabel}
          </Link>
        )}
      </div>
      {children}
    </section>
  );
}

/** Content that arrives after its skeleton: it fades in and settles up a few pixels. */
export function Reveal({ children, className }: { children: ReactNode; className?: string }) {
  const reduce = useReducedMotion();
  return (
    <motion.div
      className={className}
      initial={reduce ? false : { opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.25, ease: [0.2, 0.8, 0.2, 1] }}
    >
      {children}
    </motion.div>
  );
}

/** The grey card shapes shown while a row loads. */
export function RowSkeleton({ card, count = 4 }: { card: string; count?: number }) {
  return (
    <div className="flex gap-3 overflow-hidden px-4 md:gap-4 md:px-8" aria-hidden="true" data-scroll-row="">
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className={cn("shrink-0 animate-pulse bg-surface", card)} />
      ))}
    </div>
  );
}
