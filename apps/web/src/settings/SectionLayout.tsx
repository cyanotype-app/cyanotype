// The frame of a multi-section area (Settings, Administration). On a phone: a grouped list of sections (the index), and
// each section as its own page with a back chevron. On a desktop: the section list on the left, the section on the right.
import { motion, useReducedMotion } from "motion/react";
import { ChevronRight } from "lucide-react";
import type { ReactNode } from "react";
import { Link, NavLink, Navigate, Outlet, useLocation } from "react-router-dom";
import type { LucideIcon } from "lucide-react";
import { PageHeader } from "../shell/PageHeader";
import { cn } from "../ui/cn";
import { useDesktop } from "../ui/useMedia";

export interface AreaSection {
  /** Path under the area's root. */
  to: string;
  label: string;
  icon: LucideIcon;
  /** One line under the label in the phone list. */
  hint: string;
}

export interface AreaGroup {
  /** Small heading above the group in the lists; the first group may go without. */
  title?: string;
  sections: readonly AreaSection[];
}

interface AreaProps {
  /** `/settings`. */
  root: string;
  /** "Settings": the home page's title, and the desktop header's. */
  title: string;
  groups: readonly AreaGroup[];
  /** Names the section list for assistive technology. */
  navLabel: string;
  /** A wider content column on a desktop (tables). */
  wide?: boolean;
}

const flat = (groups: readonly AreaGroup[]) => groups.flatMap((g) => g.sections);

function currentSection(groups: readonly AreaGroup[], root: string, pathname: string): AreaSection | undefined {
  return flat(groups).find((s) => pathname === `${root}/${s.to}` || pathname.startsWith(`${root}/${s.to}/`));
}

/** A section's content fades and rises in as the section changes. */
export function Enter({ children }: { children: ReactNode }) {
  const reduce = useReducedMotion();
  return (
    <motion.div
      initial={reduce ? false : { opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.2, 0.8, 0.2, 1] }}
    >
      {children}
    </motion.div>
  );
}

export function AreaLayout({ root, title, groups, navLabel, wide }: AreaProps) {
  const desktop = useDesktop();
  const { pathname } = useLocation();
  const current = currentSection(groups, root, pathname);
  // An address the area does not know: its own not-found page, with no header of ours around it.
  if (!current && pathname !== root && pathname !== `${root}/`) return <Outlet />;

  if (!desktop) {
    return (
      <>
        <PageHeader title={current ? current.label : title} back={current ? { to: root, label: title } : undefined} />
        <div className="px-4 pt-2 pb-10">
          <Enter key={current?.to ?? "home"}>
            <Outlet />
          </Enter>
        </div>
      </>
    );
  }
  return (
    <>
      <PageHeader title={title} />
      <div className="grid grid-cols-[13.5rem_minmax(0,1fr)] gap-10 px-8 pt-3 pb-12">
        <nav aria-label={navLabel} className="sticky top-24 flex max-h-[calc(100dvh-8rem)] flex-col gap-5 self-start overflow-y-auto">
          {groups.map((g, i) => (
            <div key={g.title ?? i} className="flex flex-col gap-0.5">
              {g.title && <p className="px-2.5 pb-1 text-xs font-medium text-muted">{g.title}</p>}
              {g.sections.map((s) => {
                const Icon = s.icon;
                return (
                  <NavLink
                    key={s.to}
                    to={`${root}/${s.to}`}
                    className={({ isActive }) =>
                      cn(
                        "flex h-9 items-center gap-3 rounded-[0.6rem] px-2.5 text-[0.9375rem] text-fg no-underline transition-colors",
                        "hover:bg-surface [&_svg]:size-[1.1rem] [&_svg]:text-muted",
                        isActive && "bg-surface font-medium [&_svg]:text-accent",
                      )
                    }
                  >
                    <Icon aria-hidden="true" />
                    {s.label}
                  </NavLink>
                );
              })}
            </div>
          ))}
        </nav>
        <div className={cn("min-w-0", wide ? "max-w-5xl" : "max-w-3xl")}>
          {current && (
            <h2 className="mb-5 text-[1.375rem] font-semibold tracking-tight">{current.label}</h2>
          )}
          <Enter key={current?.to ?? "home"}>
            <Outlet />
          </Enter>
        </div>
      </div>
    </>
  );
}

/** The index route of an area: the list of sections on a phone; on a desktop it goes straight to the first section. */
export function AreaHome({ root, groups, top }: { root: string; groups: readonly AreaGroup[]; top?: ReactNode }) {
  const desktop = useDesktop();
  if (desktop) return <Navigate to={`${root}/${flat(groups)[0]!.to}`} replace />;
  return (
    <div className="flex flex-col gap-7">
      {top}
      {groups.map((g, i) => (
        <section key={g.title ?? i} aria-label={g.title ?? undefined}>
          {g.title && <h2 className="mb-1.5 px-1 text-[0.8125rem] font-medium text-muted">{g.title}</h2>}
          <ul className="divide-y divide-line overflow-hidden rounded-card bg-card ring-1 ring-line">
            {g.sections.map((s) => {
              const Icon = s.icon;
              return (
                <li key={s.to}>
                  <Link
                    to={`${root}/${s.to}`}
                    className="flex min-h-14 items-center gap-3.5 px-4 py-2 text-fg no-underline transition-colors active:bg-surface"
                  >
                    <span className="inline-flex size-8 shrink-0 items-center justify-center rounded-lg bg-accent/12 text-accent">
                      <Icon className="size-[1.1rem]" aria-hidden="true" />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block text-[0.9375rem]">{s.label}</span>
                      <span className="block truncate text-[0.8125rem] text-muted">{s.hint}</span>
                    </span>
                    <ChevronRight className="size-4 shrink-0 text-muted" aria-hidden="true" />
                  </Link>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}
