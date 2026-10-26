import { ChevronLeft, ChevronRight } from "lucide-react";
import { useReducedMotion } from "motion/react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "../ui/cn";

const arrow =
  "absolute top-[calc(50%-1rem)] z-[1] hidden size-10 items-center justify-center rounded-full bg-card text-fg shadow-float " +
  "transition-opacity duration-150 md:flex focus-visible:opacity-100 group-hover/row:opacity-100 group-focus-within/row:opacity-100 opacity-0";

/**
 * A horizontally scrolling row of cards: it snaps on touch, bleeds to the page edges (its padding lines the first card up
 * with the page's text; `wrap`: on a wide screen it wraps into rows instead) and, with a pointer, shows arrows at its ends while the mouse is over it. The row is a list; give
 * every card an `<li>` that does not shrink (`shrink-0 snap-start`).
 */
export function ScrollRow({ label, children, className, wrap }: { label: string; children: ReactNode; className?: string; wrap?: boolean }) {
  const list = useRef<HTMLUListElement>(null);
  const reduce = useReducedMotion();
  const [more, setMore] = useState({ start: false, end: false });

  const measure = useCallback(() => {
    const el = list.current;
    if (!el) return;
    const start = el.scrollLeft > 4;
    const end = el.scrollLeft + el.clientWidth < el.scrollWidth - 4;
    setMore((m) => (m.start === start && m.end === end ? m : { start, end }));
  }, []);

  useEffect(() => {
    measure();
    const el = list.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure, children]);

  const go = (dir: -1 | 1) => {
    const el = list.current;
    if (el) el.scrollBy({ left: dir * el.clientWidth * 0.8, behavior: reduce ? "auto" : "smooth" });
  };

  return (
    <div className={cn("group/row relative", className)}>
      <ul
        ref={list}
        aria-label={label}
        data-scroll-row=""
        onScroll={measure}
        className={cn(
          "flex snap-x snap-mandatory scroll-px-4 gap-3 overflow-x-auto overscroll-x-contain px-4 pb-1 [scrollbar-width:none] md:scroll-px-8 md:snap-proximity md:gap-4 md:px-8 [&::-webkit-scrollbar]:hidden",
          wrap && "lg:snap-none lg:flex-wrap lg:overflow-x-visible lg:gap-y-5",
        )}
      >
        {children}
      </ul>
      {more.start && (
        <button type="button" aria-label={`Scroll ${label} back`} onClick={() => go(-1)} className={cn(arrow, "left-4")}>
          <ChevronLeft className="size-5" aria-hidden="true" />
        </button>
      )}
      {more.end && (
        <button type="button" aria-label={`Scroll ${label} forward`} onClick={() => go(1)} className={cn(arrow, "right-4")}>
          <ChevronRight className="size-5" aria-hidden="true" />
        </button>
      )}
    </div>
  );
}
