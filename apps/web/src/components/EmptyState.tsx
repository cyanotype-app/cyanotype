import { motion, useReducedMotion } from "motion/react";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

/** An empty page: a round icon, one line of title, one line of hint and the primary action(s) beneath. */
export function EmptyState({
  icon: Icon,
  title,
  hint,
  children,
  footnote,
}: {
  icon: LucideIcon;
  title: string;
  hint: string;
  children?: ReactNode;
  footnote?: ReactNode;
}) {
  const reduce = useReducedMotion();
  return (
    <motion.div
      initial={reduce ? false : { opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.25, ease: [0.2, 0.8, 0.2, 1] }}
      className="mx-auto flex max-w-sm flex-col items-center px-6 pt-[14vh] pb-10 text-center"
      data-empty=""
    >
      <span className="mb-5 grid size-16 grid-cols-1 place-items-center rounded-full bg-surface text-accent">
        <Icon className="size-7" strokeWidth={1.6} aria-hidden="true" />
      </span>
      <h2 className="text-[1.1875rem] font-semibold tracking-tight text-balance">{title}</h2>
      <p className="mt-1.5 text-[0.9375rem] text-pretty text-muted">{hint}</p>
      {children && <div className="mt-6 flex flex-wrap items-center justify-center gap-2">{children}</div>}
      {footnote && <p className="mt-6 text-[0.8125rem] text-muted">{footnote}</p>}
    </motion.div>
  );
}
