import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { fractionOf, monthAt, offsetOf, scrubMarks, type Layout } from "../timeline";

/** Height of the draggable handle in px; the marks and the handle share one coordinate system. */
const HANDLE = 30;
/** Finger / pointer travel below this is a tap, not a drag. */
const TAP_SLOP = 4;

/** Height in px of the sticky top bar (0 when it scrolls away, as on phones). */
export function stickyTop(): number {
  const bar = document.querySelector<HTMLElement>(".topbar");
  if (!bar || getComputedStyle(bar).position !== "sticky") return 0;
  return bar.getBoundingClientRect().height;
}

interface Props {
  layout: Layout;
  /** Distance from the top of the document to the top of the list. */
  scrollMargin: number;
  /** True from the moment the handle is grabbed until it is let go: the page should not fetch photos meanwhile. */
  onDragging?: (dragging: boolean) => void;
}

/**
 * A thin rail on the right edge: years and months of the whole library laid out like the scrollbar, a handle that
 * follows the page and can be dragged (mouse, touch or pen) or the rail tapped. While dragging, a bubble names the
 * month under the handle. It only moves the window's scroll position; the timeline loads whatever lands on screen.
 */
export function Scrubber({ layout, scrollMargin, onDragging }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  const handleRef = useRef<HTMLDivElement>(null);
  const [track, setTrack] = useState(() => (typeof window === "undefined" ? 600 : Math.max(200, window.innerHeight - 140)));
  const [viewport, setViewport] = useState(() => (typeof window === "undefined" ? 800 : window.innerHeight));
  const [drag, setDrag] = useState<{ label: string; y: number } | null>(null);
  const [current, setCurrent] = useState<string>(layout.months[0]?.label ?? "");
  const live = useRef({ layout, scrollMargin, viewport });
  useLayoutEffect(() => {
    live.current = { layout, scrollMargin, viewport };
  });
  const gesture = useRef<{ startY: number; moved: boolean; onHandle: boolean } | null>(null);

  const isDragging = drag !== null;
  useEffect(() => {
    onDragging?.(isDragging);
  }, [isDragging, onDragging]);

  const travel = Math.max(1, track - HANDLE);
  const marks = useMemo(() => scrubMarks(layout, viewport - stickyTopSafe(), travel), [layout, viewport, travel]);

  // Track size follows the window (and the fixed bars that shrink it).
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
    const { layout: l, scrollMargin: margin, viewport: vp } = live.current;
    // Near the top of the page the rail sits lower than where it sticks: shorten it so it never runs off the screen.
    const rail = trackRef.current;
    if (rail) {
      const stuckAt = parseFloat(getComputedStyle(rail).top) || 0;
      const shrink = Math.max(0, Math.round(rail.getBoundingClientRect().top - stuckAt));
      if (rail.style.getPropertyValue("--shrink") !== `${shrink}px`) rail.style.setProperty("--shrink", `${shrink}px`);
    }
    const sticky = stickyTop();
    const vh = vp - sticky;
    const offset = window.scrollY + sticky - margin;
    const f = fractionOf(offset, l.height, vh);
    const y = f * Math.max(1, (trackRef.current?.clientHeight || track) - HANDLE);
    if (handleRef.current) handleRef.current.style.transform = `translateY(${y}px)`;
    const m = monthAt(l, offset + 1);
    if (m) setCurrent((c) => (c === m.label ? c : m.label));
  }, [track]);

  // Keep the handle on the page's position without re-rendering on every scroll event.
  useEffect(() => {
    let raf = 0;
    const onScroll = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(place);
    };
    place();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("scroll", onScroll);
    };
  }, [place, layout, scrollMargin, viewport]);

  const scrollToOffset = useCallback((offset: number) => {
    const { scrollMargin: margin } = live.current;
    window.scrollTo(0, Math.max(0, offset + margin - stickyTop()));
  }, []);

  const fractionAt = (clientY: number) => {
    const r = trackRef.current!.getBoundingClientRect();
    return Math.min(1, Math.max(0, (clientY - r.top - HANDLE / 2) / Math.max(1, r.height - HANDLE)));
  };
  const moveTo = (clientY: number) => {
    const { layout: l, viewport: vp } = live.current;
    const vh = vp - stickyTop();
    const f = fractionAt(clientY);
    const offset = offsetOf(f, l.height, vh);
    scrollToOffset(offset);
    const m = monthAt(l, offset + 1);
    const y = f * Math.max(1, (trackRef.current?.clientHeight || track) - HANDLE);
    if (handleRef.current) handleRef.current.style.transform = `translateY(${y}px)`;
    if (m) setDrag({ label: m.label, y });
  };

  const jumpToMonth = (month: string) => {
    const m = live.current.layout.months.find((x) => x.month === month);
    if (m) scrollToOffset(m.top);
  };

  const step = (delta: number) => {
    const { layout: l, scrollMargin: margin } = live.current;
    const offset = window.scrollY + stickyTop() - margin;
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

  return (
    <div
      className={`scrubber${drag ? " dragging" : ""}`}
      ref={trackRef}
      role="group"
      aria-label="Jump to a date"
      onPointerDown={(e) => {
        if (e.button !== 0 && e.pointerType === "mouse") return;
        e.currentTarget.setPointerCapture?.(e.pointerId);
        const onHandle = !!(e.target as HTMLElement).closest(".scrubber-handle");
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
        setDrag(null);
        if (!g || g.moved || g.onHandle) return;
        // A tap: a labelled month goes to exactly its header, anywhere else to that point of the library.
        const month = (e.target as HTMLElement).closest<HTMLElement>("[data-month]")?.dataset.month;
        if (month) jumpToMonth(month);
        else moveTo(e.clientY);
        setDrag(null);
      }}
      onPointerCancel={() => {
        gesture.current = null;
        setDrag(null);
      }}
    >
      <div className="scrubber-marks" aria-hidden="true">
        {marks.map((m) => (
          <span
            key={m.month}
            className={m.year ? "scrub-year" : "scrub-tick"}
            data-month={m.month}
            title={m.label}
            style={{ top: m.y + HANDLE / 2 }}
          >
            {m.year}
          </span>
        ))}
      </div>
      <div
        className="scrubber-handle"
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
        onKeyDown={(e) => {
          if (e.key === "ArrowUp" || e.key === "PageUp") step(e.key === "PageUp" ? -12 : -1);
          else if (e.key === "ArrowDown" || e.key === "PageDown") step(e.key === "PageDown" ? 12 : 1);
          else if (e.key === "Home") scrollToOffset(0);
          else if (e.key === "End") scrollToOffset(layout.height);
          else return;
          e.preventDefault();
        }}
      >
        <span className="scrubber-grip" aria-hidden="true" />
      </div>
      {drag && (
        <div
          className="scrubber-bubble"
          role="status"
          style={{ top: drag.y + HANDLE / 2 }}
        >
          {drag.label}
        </div>
      )}
    </div>
  );
}

const stickyTopSafe = () => (typeof document === "undefined" ? 0 : stickyTop());

