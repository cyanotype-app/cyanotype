import { animate, motion, useDragControls, useMotionValue, useReducedMotion, type PanInfo } from "motion/react";
import { useCallback, useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { useDesktop } from "../ui/useMedia";
import { cn } from "../ui/cn";

export type Snap = "peek" | "half" | "full";

/** How much of the panel shows folded (its handle and the count), and how much of the map it covers at "half". */
const PEEK_PX = 92;
const HALF_SHARE = 0.55;
/** The header the full panel stops under. */
const HEADER_PX = 76;

/**
 * The photos of the map's view. On a desktop a side panel floating at the map's right; on a phone a sheet over the
 * map's bottom that the finger moves between folded (just the count), half and full height. It is not modal: the map
 * stays alive behind it, which is why it is not the app's `Sheet`. `children` is the list; `heading` the count above it.
 */
export function MapPanel({
  heading,
  label,
  listRef,
  children,
  open,
}: {
  heading: ReactNode;
  label: string;
  listRef: RefObject<HTMLDivElement | null>;
  children: ReactNode;
  /** Start unfolded to full height (the map cannot be drawn, so the list is the page). */
  open?: boolean;
}) {
  const desktop = useDesktop();
  if (desktop)
    return (
      <aside
        aria-label={label}
        className="absolute top-(--hdr) right-3 bottom-3 z-[4] flex w-[22rem] flex-col overflow-hidden rounded-sheet bg-card shadow-float ring-1 ring-line"
      >
        <div className="shrink-0 px-4 pt-3.5 pb-2">{heading}</div>
        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-1 pb-3">
          {children}
        </div>
      </aside>
    );
  return (
    <SheetPanel heading={heading} label={label} listRef={listRef} open={open}>
      {children}
    </SheetPanel>
  );
}

function SheetPanel({
  heading,
  label,
  listRef,
  children,
  open,
}: {
  heading: ReactNode;
  label: string;
  listRef: RefObject<HTMLDivElement | null>;
  children: ReactNode;
  open?: boolean;
}) {
  const frame = useRef<HTMLDivElement>(null);
  const reduce = useReducedMotion();
  const drag = useDragControls();
  const y = useMotionValue(0);
  const [height, setHeight] = useState(0);
  const [snap, setSnap] = useState<Snap>(open ? "full" : "peek");
  const placed = useRef(false);

  useEffect(() => {
    const el = frame.current;
    if (!el) return;
    const read = () => setHeight(el.clientHeight);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const panel = Math.max(0, height - HEADER_PX);
  const at = useCallback(
    (s: Snap) => (s === "full" ? 0 : s === "half" ? Math.max(0, panel - height * HALF_SHARE) : Math.max(0, panel - PEEK_PX)),
    [height, panel],
  );

  const go = useCallback(
    (s: Snap) => {
      setSnap(s);
      if (reduce) y.set(at(s));
      else void animate(y, at(s), { type: "spring", stiffness: 460, damping: 44, mass: 0.9 });
    },
    [at, reduce, y],
  );

  // Put the panel where its snap says: at once the first time it is measured, softly after (a rotation, a taller window).
  useEffect(() => {
    if (height === 0) return;
    if (!placed.current) {
      placed.current = true;
      y.set(at(snap));
      return;
    }
    y.set(at(snap));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [height]);

  useEffect(() => {
    if (open && height > 0) go("full");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, height > 0]);

  const onDragEnd = (_: unknown, info: PanInfo) => {
    const projected = y.get() + info.velocity.y * 0.18;
    const options: Snap[] = ["full", "half", "peek"];
    const nearest = options.reduce((best, s) => (Math.abs(at(s) - projected) < Math.abs(at(best) - projected) ? s : best));
    go(nearest);
  };

  return (
    <div ref={frame} className="pointer-events-none absolute inset-0 z-[4]">
      <motion.section
        aria-label={label}
        style={{ y, height: height ? panel : "calc(100% - 4.75rem)" }}
        drag="y"
        dragControls={drag}
        dragListener={false}
        dragConstraints={{ top: 0, bottom: at("peek") }}
        dragElastic={{ top: 0.05, bottom: 0.2 }}
        dragMomentum={false}
        onDragEnd={onDragEnd}
        className="pointer-events-auto absolute inset-x-0 bottom-0 flex flex-col rounded-t-sheet bg-card shadow-float ring-1 ring-line"
      >
        <div className="shrink-0 touch-none px-4 pt-1.5 pb-2" onPointerDown={(e) => drag.start(e)}>
          <button
            type="button"
            aria-expanded={snap !== "peek"}
            aria-label={snap === "peek" ? "Show photos in view" : "Fold photos in view"}
            onClick={() => go(snap === "peek" ? "half" : "peek")}
            className="mx-auto mb-1.5 flex h-5 w-16 items-center justify-center rounded-full"
          >
            <span className="h-1.5 w-10 rounded-full bg-line" aria-hidden="true" />
          </button>
          {heading}
        </div>
        <div
          ref={listRef}
          className={cn("min-h-0 flex-1 overscroll-contain px-0.5 pb-4", snap === "peek" ? "overflow-hidden" : "overflow-y-auto")}
        >
          {children}
        </div>
      </motion.section>
    </div>
  );
}
