import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { cn } from "../ui/cn";
import { fractionOf, monthAt, offsetOf, scrubMarks, type Layout } from "../timeline";
import { headerHeight, useHeaderHeight } from "./headerHeight";

/** Height of the draggable handle in px; the marks and the handle share one coordinate system. */
const HANDLE = 44;
/** Finger / pointer travel below this is a tap, not a drag. */
const TAP_SLOP = 4;
/** How long the rail stays after the page stops scrolling. */
const LINGER_MS = 1300;
/** A mouse this close to the right edge wakes the rail. */
const EDGE_PX = 64;

/** Height in px of the sticky header the rail and the pinned month sit below (0 where the page has none). */
export const stickyTop = headerHeight;

interface Props {
  layout: Layout;
  /** Distance from the top of the document to the top of the list. */
  scrollMargin: number;
  /** True from the moment the handle is grabbed until it is let go: the page should not fetch photos meanwhile. */
  onDragging?: (dragging: boolean) => void;
  /** Not shown on a phone (the selection bar takes the bottom, and dragging over the rail would fight selecting). */
  hideOnPhone?: boolean;
}

/**
 * A thin rail on the right edge, floating over the photos: it appears while the page scrolls (or when the mouse nears the
 * edge), a handle follows the page and can be dragged (touch, mouse or pen) and, once awake, a tap on a year or month jumps
 * there. While dragging, a bubble names the month under the handle. It only moves the window's scroll position; the timeline
 * loads whatever lands on screen.
 */
export function Scrubber({ layout, scrollMargin, onDragging, hideOnPhone }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  const handleRef = useRef<HTMLDivElement>(null);
  const bubbleRef = useRef<HTMLDivElement>(null);
  const hdr = useHeaderHeight();
  const [track, setTrack] = useState(() => (typeof window === "undefined" ? 600 : Math.max(200, window.innerHeight - 140)));
  const [viewport, setViewport] = useState(() => (typeof window === "undefined" ? 800 : window.innerHeight));
  const [dragLabel, setDragLabel] = useState<string | null>(null);
  const [current, setCurrent] = useState<string>(layout.months[0]?.label ?? "");
  const [scrolling, setScrolling] = useState(false);
  const [near, setNear] = useState(false);
  const [focused, setFocused] = useState(false);
  const live = useRef({ layout, scrollMargin, viewport, hdr });
  useLayoutEffect(() => {
    live.current = { layout, scrollMargin, viewport, hdr };
  });
  const gesture = useRef<{ startY: number; moved: boolean; onHandle: boolean } | null>(null);

  const dragging = dragLabel !== null;
  useEffect(() => {
    onDragging?.(dragging);
  }, [dragging, onDragging]);

  const travel = Math.max(1, track - HANDLE);
  const marks = useMemo(() => scrubMarks(layout, viewport - hdr, travel), [layout, viewport, hdr, travel]);

  // Track size follows the window (and the bars that shrink it).
  useLayoutEffect(() => {
    const el = trackRef.current;
    if (!el) return;
    const measure = () => {
      setTrack(el.clientHeight || track);
      setViewport(window.innerHeight);
    };
    measure();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(el);
    window.addEventListener("resize", measure);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", measure);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const place = useCallback(() => {
    const { layout: l, scrollMargin: margin, viewport: vp, hdr: top } = live.current;
    const vh = vp - top;
    const offset = window.scrollY + top - margin;
    const f = fractionOf(offset, l.height, vh);
    const y = f * Math.max(1, (trackRef.current?.clientHeight || track) - HANDLE);
    if (handleRef.current) handleRef.current.style.transform = `translateY(${y}px)`;
    const m = monthAt(l, offset + 1);
    if (m) setCurrent((c) => (c === m.label ? c : m.label));
  }, [track]);

  // Keep the handle on the page's position without re-rendering on every scroll event; wake the rail while scrolling.
  useEffect(() => {
    let raf = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onScroll = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(place);
      setScrolling(true);
      clearTimeout(timer);
      timer = setTimeout(() => setScrolling(false), LINGER_MS);
    };
    place();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(raf);
      clearTimeout(timer);
      window.removeEventListener("scroll", onScroll);
    };
  }, [place, layout, scrollMargin, viewport, hdr]);

  // A mouse near the right edge wakes the rail; touch wakes it by scrolling.
  useEffect(() => {
    const onMove = (e: PointerEvent) => {
      if (e.pointerType !== "mouse" || gesture.current) return;
      const el = trackRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      setNear(e.clientX >= window.innerWidth - EDGE_PX && e.clientY >= r.top && e.clientY <= r.bottom);
    };
    window.addEventListener("pointermove", onMove, { passive: true });
    return () => window.removeEventListener("pointermove", onMove);
  }, []);

  const scrollToOffset = useCallback((offset: number) => {
    const { scrollMargin: margin, hdr: top } = live.current;
    window.scrollTo(0, Math.max(0, offset + margin - top));
  }, []);

  const fractionAt = (clientY: number) => {
    const r = trackRef.current!.getBoundingClientRect();
    return Math.min(1, Math.max(0, (clientY - r.top - HANDLE / 2) / Math.max(1, r.height - HANDLE)));
  };
  const moveTo = (clientY: number) => {
    const { layout: l, viewport: vp, hdr: top } = live.current;
    const f = fractionAt(clientY);
    const offset = offsetOf(f, l.height, vp - top);
    scrollToOffset(offset);
    const m = monthAt(l, offset + 1);
    const y = f * Math.max(1, (trackRef.current?.clientHeight || track) - HANDLE);
    if (handleRef.current) handleRef.current.style.transform = `translateY(${y}px)`;
    if (bubbleRef.current) bubbleRef.current.style.transform = `translateY(${y + HANDLE / 2}px) translateY(-50%)`;
    if (m) setDragLabel(m.label);
  };

  const jumpToMonth = (month: string) => {
    const m = live.current.layout.months.find((x) => x.month === month);
    if (m) scrollToOffset(m.top);
  };

  const step = (delta: number) => {
    const { layout: l, scrollMargin: margin, hdr: top } = live.current;
    const offset = window.scrollY + top - margin;
    const here = monthAt(l, offset + 1);
    if (!here) return;
    const i = Math.min(l.months.length - 1, Math.max(0, l.months.indexOf(here) + delta));
    // Inside a long month Up first goes to its start.
    const target = delta < 0 && offset > here.top + 2 ? here : l.months[i]!;
    scrollToOffset(target.top);
  };

  const { height, months } = layout;
  // Nothing to scrub in a library that fits on a couple of screens.
  if (months.length < 2 || height < viewport * 2.5) return null;

  const awake = dragging || scrolling || near || focused;
  const railLive = dragging || near || focused;
  return (
    <div
      className={cn(
        "pointer-events-none fixed right-0 z-[6] w-16 select-none",
        hideOnPhone && "max-md:hidden",
        railLive && "pointer-events-auto",
      )}
      style={{ top: hdr + 6, bottom: "calc(var(--tabbar-h, 0px) + 6px)" }}
      data-scrubber=""
      data-awake={awake ? "" : undefined}
      ref={trackRef}
      role="group"
      aria-label="Jump to a date"
      onPointerDown={(e) => {
        if (e.button !== 0 && e.pointerType === "mouse") return;
        const onHandle = !!(e.target as HTMLElement).closest("[data-handle]");
        if (!onHandle && !railLive) return;
        e.currentTarget.setPointerCapture?.(e.pointerId);
        gesture.current = { startY: e.clientY, moved: false, onHandle };
        if (onHandle) moveTo(e.clientY);
      }}
      onPointerMove={(e) => {
        const g = gesture.current;
        if (!g) return;
        if (!g.moved && Math.abs(e.clientY - g.startY) > TAP_SLOP) g.moved = true;
        if (g.moved || g.onHandle) moveTo(e.clientY);
      }}
      onPointerUp={(e) => {
        const g = gesture.current;
        gesture.current = null;
        setDragLabel(null);
        if (!g || g.moved || g.onHandle) return;
        // A tap: a labelled month goes to exactly its header, anywhere else to that point of the library.
        const month = (e.target as HTMLElement).closest<HTMLElement>("[data-month]")?.dataset.month;
        if (month) jumpToMonth(month);
        else moveTo(e.clientY);
        setDragLabel(null);
      }}
      onPointerCancel={() => {
        gesture.current = null;
        setDragLabel(null);
      }}
    >
      <div
        className={cn("absolute inset-y-0 right-0 transition-opacity duration-200 motion-reduce:transition-none", awake ? "opacity-100" : "opacity-0")}
        aria-hidden="true"
      >
        {marks.map((m) => (
          <span
            key={m.month}
            data-month={m.month}
            data-year={m.year ? "" : undefined}
            title={m.label}
            className={cn(
              "absolute -translate-y-1/2 select-none",
              m.year
                ? "right-5 rounded-full bg-black/50 px-1.5 text-[0.6875rem] leading-[1.35] font-semibold text-white backdrop-blur-sm"
                : "right-2.5 h-0.5 w-1.5 rounded-full bg-white/80 shadow-[0_0_2px_rgb(0_0_0/0.5)]",
            )}
            style={{ top: m.y + HANDLE / 2 }}
          >
            {m.year}
          </span>
        ))}
      </div>
      <div
        data-handle=""
        ref={handleRef}
        role="slider"
        tabIndex={0}
        aria-label="Scroll through your library by date"
        aria-orientation="vertical"
        aria-valuemin={0}
        aria-valuemax={months.length - 1}
        aria-valuenow={Math.max(0, months.findIndex((m) => m.label === current))}
        aria-valuetext={current}
        style={{ height: HANDLE }}
        className={cn(
          "absolute top-0 right-0 flex w-11 touch-none items-center justify-end pr-1 outline-none transition-opacity duration-200 motion-reduce:transition-none",
          awake ? "pointer-events-auto opacity-100" : "pointer-events-none opacity-0",
        )}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onKeyDown={(e) => {
          if (e.key === "ArrowUp" || e.key === "PageUp") step(e.key === "PageUp" ? -12 : -1);
          else if (e.key === "ArrowDown" || e.key === "PageDown") step(e.key === "PageDown" ? 12 : 1);
          else if (e.key === "Home") scrollToOffset(0);
          else if (e.key === "End") scrollToOffset(layout.height);
          else return;
          e.preventDefault();
        }}
      >
        <span
          aria-hidden="true"
          className={cn(
            "h-9 rounded-full bg-white shadow-[0_0_0_1px_rgb(0_0_0/0.18),0_2px_6px_rgb(0_0_0/0.3)] transition-[width] duration-150",
            dragging || focused ? "w-2" : "w-1.5",
            focused && "outline-2 outline-offset-2 outline-focus",
          )}
        />
      </div>
      <div
        ref={bubbleRef}
        role="status"
        className={cn(
          "pointer-events-none absolute top-0 right-12 rounded-full bg-black/75 px-3 py-1 text-sm font-medium whitespace-nowrap text-white backdrop-blur-sm transition-opacity duration-150",
          dragLabel ? "opacity-100" : "opacity-0",
        )}
      >
        {dragLabel}
      </div>
    </div>
  );
}
