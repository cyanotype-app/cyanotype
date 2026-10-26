import { motion, useReducedMotion } from "motion/react";
import { MoreHorizontal } from "lucide-react";
import { useEffect, useRef, type ReactNode } from "react";
import { cn } from "../ui/cn";
import { Menu, MenuItem } from "../ui/Menu";
import { useDesktop } from "../ui/useMedia";

/** One thing the selection bar can do: an icon button with a label, or a row of its "⋯" menu when there is no room. */
export interface BarAction {
  id: string;
  label: string;
  icon: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  /** Destructive: red. */
  danger?: boolean;
  /** Always shown on the bar, after the "⋯" menu (trash). */
  pinned?: boolean;
  title?: string;
  /** The button's name for assistive technology when it is fuller than the short label under the icon. */
  ariaLabel?: string;
}

interface Props {
  count: number;
  /** How many items could be selected (the loaded ones); "Select all" is disabled when everything is selected. */
  total: number;
  onSelectAll: () => void;
  onClear: () => void;
  /** Leaves selection mode. Omit where selection is the page's only mode (Trash). */
  onDone?: () => void;
  /** The actions for the selection, as icon buttons that overflow into a "⋯" menu. */
  actions?: BarAction[];
  /** Free-form buttons for pages that lay out their own (they sit in a row beside the count). */
  children?: ReactNode;
  /** A line under the count, for what the selection includes beyond the tiles that are ticked (photos inside bursts). */
  note?: string | null;
  /** What the bar says while nothing is selected (default "Select photos"). */
  emptyLabel?: string;
}

/** Slots for actions (the "⋯" button counts as one): what fits across a phone, and across the desktop pill. */
const SLOTS_PHONE = 5;
const SLOTS_DESKTOP = 8;

/**
 * Shown while selecting. On a phone it covers the tab bar with a frosted bar: the count and Select all / Clear / Done above
 * a row of icon buttons with labels, the rest in a "⋯" menu. On a desktop it is a floating pill at the bottom. It slides in
 * and out, and tells the toasts how high it is (`--selbar-h`) so they stay above it.
 */
export function SelectionBar({ count, total, onSelectAll, onClear, onDone, actions, children, note = null, emptyLabel = "Select photos" }: Props) {
  const desktop = useDesktop();
  const reduce = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const publish = () => document.documentElement.style.setProperty("--selbar-h", `${el.offsetHeight + (desktop ? 24 : 0)}px`);
    publish();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(publish);
    ro?.observe(el);
    return () => {
      ro?.disconnect();
      document.documentElement.style.removeProperty("--selbar-h");
    };
  }, [desktop]);

  const slots = desktop ? SLOTS_DESKTOP : SLOTS_PHONE;
  const list = actions ?? [];
  const pinned = list.filter((a) => a.pinned);
  const rest = list.filter((a) => !a.pinned);
  const room = slots - pinned.length;
  const overflow = rest.length > room ? rest.slice(Math.max(0, room - 1)) : [];
  const inline = rest.length > room ? rest.slice(0, Math.max(0, room - 1)) : rest;

  return (
    <motion.div
      ref={ref}
      role="toolbar"
      aria-label="Selection"
      data-selection-bar=""
      initial={reduce ? { opacity: 0 } : { opacity: 0, y: 72 }}
      animate={reduce ? { opacity: 1 } : { opacity: 1, y: 0 }}
      exit={reduce ? { opacity: 0 } : { opacity: 0, y: 72 }}
      transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 420, damping: 38 }}
      className={cn(
        "fixed z-20 bg-bg text-fg",
        "inset-x-0 bottom-0 border-t border-line pb-[env(safe-area-inset-bottom,0px)]",
        "md:inset-x-auto md:bottom-6 md:left-[calc(50%+7.75rem)] md:-translate-x-1/2 md:max-w-[calc(100vw-17rem)] md:rounded-full md:border md:pb-0 md:shadow-float",
        "md:glass",
      )}
    >
      <div className="flex flex-col md:flex-row md:items-center md:gap-1 md:py-1.5 md:pr-2 md:pl-5">
        <div className="flex min-h-11 items-center gap-1 px-4 md:min-h-0 md:px-0">
          <span className="mr-auto text-[0.9375rem] font-semibold md:mr-2 md:min-w-24" aria-live="polite">
            {count === 0 ? emptyLabel : `${count.toLocaleString("en-US")} selected`}
            {note && <span className="block text-xs font-normal text-muted">{note}</span>}
          </span>
          <TextButton onClick={onSelectAll} disabled={total === 0 || count === total}>
            Select all
          </TextButton>
          <TextButton onClick={onClear} disabled={count === 0}>
            Clear
          </TextButton>
          {onDone && (
            <TextButton onClick={onDone} strong>
              Done
            </TextButton>
          )}
        </div>
        {(list.length > 0 || children) && <span className="mx-1 hidden h-7 w-px bg-line md:block" aria-hidden="true" />}
        {list.length > 0 && (
          <div className="flex items-stretch justify-around px-1 pb-1 md:justify-start md:gap-0.5 md:p-0">
            {inline.map((a) => (
              <ActionButton key={a.id} action={a} />
            ))}
            {overflow.length > 0 && (
              <Menu
                align="end"
                label="More actions"
                trigger={
                  <button type="button" className={actionClass} aria-label="More actions">
                    <MoreHorizontal aria-hidden="true" />
                    <span className={labelClass}>More</span>
                  </button>
                }
              >
                {overflow.map((a) => (
                  <MenuItem key={a.id} disabled={a.disabled} danger={a.danger} onSelect={a.onSelect}>
                    {a.icon}
                    {a.label}
                  </MenuItem>
                ))}
              </Menu>
            )}
            {pinned.map((a) => (
              <ActionButton key={a.id} action={a} />
            ))}
          </div>
        )}
        {children && <div className="flex flex-wrap items-center gap-1 px-3 pb-2 md:p-0">{children}</div>}
      </div>
    </motion.div>
  );
}

const actionClass =
  "flex min-h-14 min-w-14 flex-1 flex-col items-center justify-center gap-0.5 rounded-xl px-1 text-fg transition-colors md:min-h-12 md:flex-none md:px-2.5 " +
  "hover:bg-surface active:scale-[0.97] disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-[1.35rem]";
const labelClass = "text-[0.6875rem] leading-none font-medium whitespace-nowrap";

function ActionButton({ action: a }: { action: BarAction }) {
  return (
    <button type="button" className={cn(actionClass, a.danger && "text-danger")} disabled={a.disabled} title={a.title} aria-label={a.ariaLabel} onClick={a.onSelect}>
      {a.icon}
      <span className={labelClass}>{a.label}</span>
    </button>
  );
}

function TextButton({ children, onClick, disabled, strong }: { children: ReactNode; onClick: () => void; disabled?: boolean; strong?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "h-9 rounded-full px-2.5 text-sm font-medium whitespace-nowrap hover:bg-surface disabled:pointer-events-none disabled:opacity-40",
        strong ? "text-accent" : "text-muted hover:text-fg",
      )}
    >
      {children}
    </button>
  );
}
