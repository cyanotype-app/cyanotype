import { defaultRangeExtractor, useWindowVirtualizer, type Range } from "@tanstack/react-virtual";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AssetListItem } from "../api";
import { cn } from "../ui/cn";
import { useDesktop } from "../ui/useMedia";
import {
  buildLayout,
  dayTop,
  gridGeometry,
  monthAt,
  photoRange,
  rowAt,
  rowOfPhoto,
  type GridStyle,
  type Layout,
  type Loaded,
  type Row,
  type TimelineIndex,
} from "../timeline";
import { useHeaderHeight } from "./headerHeight";
import { Scrubber } from "./Scrubber";
import { Tile } from "./Tile";

/** Rows beyond the viewport that are rendered, each way. */
const OVERSCAN_ROWS = 3;
/** Photos past the last visible one to start fetching, so scrolling down rarely meets an empty tile. */
const PREFETCH = 60;
/** A pinch that changes the distance between the fingers by this factor changes the density one step. */
const PINCH_STEP = 1.28;

interface Props {
  index: TimelineIndex;
  loaded: Loaded;
  /** Photos `from..to` (global indices) are on screen or about to be. */
  onRange: (from: number, to: number) => void;
  /** A tile was opened; `index` is the photo's global index. */
  onOpen: (index: number) => void;
  onThumbError: (id: string) => void;
  selecting: boolean;
  selected: ReadonlySet<string>;
  onSelect: (id: string, shift: boolean) => void;
  onLongPress: (id: string) => void;
  /** Touch, after a long press: the finger crossed another tile / was lifted. */
  onDragOver?: (id: string) => void;
  onDragEnd?: () => void;
  onFavorite: (id: string, favorite: boolean) => void;
  /** Scroll to this day (`YYYY-MM-DD`) once when it is given or changes (a link from a photo's details). */
  jumpToDay?: string | null;
  /** Square crops or rows that keep each photo's shape; by default justified on a desktop and square on a phone. */
  gridStyle?: GridStyle;
  /** Columns to add (square) or shrink the rows by (justified); a pinch asks for a step through `onDensity`. */
  density?: number;
  onDensity?: (delta: number) => void;
}

/**
 * The library as date-grouped rows (month header, day header, rows of tiles) of which only the ones near the viewport exist
 * in the DOM. The grid spans the full width of the page with narrow gutters. Square rows have a fixed height, so the layout
 * of the whole library (and with it the scrollbar and the scrubber) is exact from the per-day counts alone; justified rows
 * are built from the photos' shapes, estimated until their page has loaded (the scroll position is held while that settles).
 * The month header of the part on screen stays pinned below the page's header. The page itself scrolls (window virtualizer),
 * so the sticky bars and the browser's scroll restoration just work.
 */
export function TimelineGrid({
  index,
  loaded,
  onRange,
  onOpen,
  onThumbError,
  selecting,
  selected,
  onSelect,
  onLongPress,
  onDragOver,
  onDragEnd,
  onFavorite,
  jumpToDay,
  gridStyle: style,
  density = 0,
  onDensity,
}: Props) {
  const desktop = useDesktop();
  const gridStyle = style ?? (desktop ? "justified" : "square");
  const listRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(() => (typeof window === "undefined" ? 1024 : window.innerWidth));
  const [scrollMargin, setScrollMargin] = useState(0);
  const hdr = useHeaderHeight();
  // While the scrubber is being dragged the page flies past months nobody looks at: fetch once it is let go.
  const [scrubbing, setScrubbing] = useState(false);

  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const measure = () => {
      if (el.clientWidth > 0) setWidth(el.clientWidth);
      setScrollMargin(Math.round(el.getBoundingClientRect().top + window.scrollY));
    };
    measure();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(el);
    // Things above the list (a notice, the processing banner) move it without resizing it.
    const above = el.closest("[data-library]");
    if (above) ro?.observe(above);
    window.addEventListener("resize", measure);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, []);
  // Justified rows depend on the photos' shapes, square ones on nothing but the counts.
  const shapes = gridStyle === "justified" ? loaded : undefined;
  const layout = useMemo(() => buildLayout(index, gridGeometry(width, gridStyle, density), shapes), [index, width, gridStyle, density, shapes]);
  const { rows } = layout;

  // When rows above the screen change height (their photos' shapes arrived, the width changed) the photo on screen stays put.
  const before = useRef<{ layout: Layout; index: TimelineIndex } | null>(null);
  useLayoutEffect(() => {
    const prev = before.current;
    before.current = { layout, index };
    if (!prev || prev.layout === layout || prev.index !== index || window.scrollY <= 0) return;
    const y = window.scrollY + hdr - scrollMargin;
    if (y <= 0) return;
    const was = prev.layout.rows[rowAt(prev.layout.rows, y)];
    if (!was) return;
    let now: Row | undefined;
    if (was.kind === "tiles") now = layout.rows[rowOfPhoto(layout, was.start)];
    else now = layout.rows.find((r) => r.key === was.key);
    if (!now) return;
    const delta = now.top + Math.min(now.height, Math.max(0, y - was.top)) - y;
    if (Math.abs(delta) >= 1) window.scrollBy(0, delta);
  }, [layout, index, hdr, scrollMargin]);

  // A link to a day: scroll there once the layout exists (the position is measured now, not from state that lags a render).
  const jumped = useRef<string | null>(null);
  useLayoutEffect(() => {
    const el = listRef.current;
    if (!jumpToDay || jumped.current === jumpToDay || !el) return;
    const top = dayTop(rows, jumpToDay);
    if (top === null) return;
    jumped.current = jumpToDay;
    window.scrollTo(0, Math.max(0, top + el.getBoundingClientRect().top + window.scrollY - hdr));
  }, [jumpToDay, rows, hdr]);

  // The month header that is pinned: the last one at or above the top of the visible part.
  const pinnedAt = useCallback(() => monthAt(layout, window.scrollY + hdr - scrollMargin)?.row ?? -1, [layout, hdr, scrollMargin]);
  const rangeExtractor = useCallback(
    (range: Range) => {
      const pinned = pinnedAt();
      const base = defaultRangeExtractor(range);
      return pinned >= 0 && !base.includes(pinned) ? [pinned, ...base].sort((a, b) => a - b) : base;
    },
    [pinnedAt],
  );
  const virtualizer = useWindowVirtualizer({
    count: rows.length,
    estimateSize: (i) => rows[i]!.height,
    getItemKey: (i) => rows[i]!.key,
    overscan: OVERSCAN_ROWS,
    scrollMargin,
    rangeExtractor,
  });
  // Sizes are exact and never measured; when the layout changes forget the cached ones.
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [layout, virtualizer]);

  const virtual = virtualizer.getVirtualItems();
  const pinnedRow = pinnedAt();
  const first = virtual.find((v) => rows[v.index]?.kind === "tiles")?.index ?? -1;
  const last = virtual.findLast((v) => rows[v.index]?.kind === "tiles")?.index ?? -1;
  useEffect(() => {
    if (first < 0 || scrubbing) return;
    const r = photoRange(layout, first, last);
    if (r) onRange(r[0], r[1] + PREFETCH);
  }, [first, last, layout, onRange, scrubbing]);

  // Pinch with two fingers: fewer, larger tiles when the fingers spread, more and smaller when they close.
  const pinch = useRef(onDensity);
  useEffect(() => {
    pinch.current = onDensity;
  });
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    let from = 0;
    const dist = (e: TouchEvent) => Math.hypot(e.touches[0]!.clientX - e.touches[1]!.clientX, e.touches[0]!.clientY - e.touches[1]!.clientY);
    const start = (e: TouchEvent) => {
      from = e.touches.length === 2 ? dist(e) : 0;
    };
    const move = (e: TouchEvent) => {
      if (e.touches.length !== 2 || !from || !pinch.current) return;
      if (e.cancelable) e.preventDefault();
      const ratio = dist(e) / from;
      if (ratio >= PINCH_STEP) {
        pinch.current(-1);
        from = dist(e);
      } else if (ratio <= 1 / PINCH_STEP) {
        pinch.current(1);
        from = dist(e);
      }
    };
    const end = () => {
      from = 0;
    };
    el.addEventListener("touchstart", start, { passive: true });
    el.addEventListener("touchmove", move, { passive: false });
    el.addEventListener("touchend", end, { passive: true });
    return () => {
      el.removeEventListener("touchstart", start);
      el.removeEventListener("touchmove", move);
      el.removeEventListener("touchend", end);
    };
  }, []);

  const { cols, gap } = layout.geometry;

  return (
    <div className="relative" data-timeline="">
      <Scrubber layout={layout} scrollMargin={scrollMargin} onDragging={setScrubbing} hideOnPhone={selecting} />
      <div
        className="relative w-full [touch-action:pan-y]"
        ref={listRef}
        style={{ height: virtualizer.getTotalSize() }}
        role="list"
        aria-label="Photos"
      >
        {virtual.map((v) => {
          const row = rows[v.index];
          if (!row) return null;
          if (row.kind === "month" || row.kind === "day") {
            const pinned = row.kind === "month" && v.index === pinnedRow;
            const style = pinned
              ? { height: row.height, top: hdr }
              : { height: row.height, transform: `translateY(${v.start - scrollMargin}px)` };
            const Heading = row.kind === "month" ? "h2" : "h3";
            return (
              <div
                key={v.key}
                role="listitem"
                data-row={row.kind}
                data-pinned={pinned ? "" : undefined}
                style={style}
                className={cn(
                  "flex w-full items-end px-4 pb-1.5 md:px-8",
                  row.kind === "month" ? (pinned ? "sticky z-[3] bg-bg/92 backdrop-blur-xl" : "absolute top-0 left-0 z-[1]") : "absolute top-0 left-0",
                )}
              >
                <Heading
                  className={cn(
                    "m-0 truncate leading-tight",
                    row.kind === "month" ? "text-[1.0625rem] font-semibold tracking-tight text-fg" : "text-[0.8125rem] font-medium text-muted",
                  )}
                >
                  {row.label}
                </Heading>
              </div>
            );
          }
          const justified = row.ratios !== undefined;
          const tileH = row.tileH ?? row.height - gap;
          const columns = justified
            ? row.ratios!.map((r) => (row.fill ? `minmax(0,${r}fr)` : `${r * tileH}px`)).join(" ")
            : `repeat(${cols}, minmax(0, 1fr))`;
          return (
            <div
              key={v.key}
              role="listitem"
              data-row="tiles"
              data-fill={(justified ? row.fill : row.count === cols) ? "" : undefined}
              className="absolute top-0 left-0 grid w-full"
              style={{ height: row.height, paddingBottom: gap, gap, gridTemplateColumns: columns, transform: `translateY(${v.start - scrollMargin}px)` }}
            >
              {Array.from({ length: row.count }, (_, k) => {
                const i = row.start + k;
                const item: AssetListItem | undefined = loaded.items.get(i);
                if (!item) return <div key={`p${i}`} className="bg-surface" aria-hidden="true" />;
                return (
                  <Tile
                    key={item.id}
                    item={item}
                    index={i}
                    onOpen={onOpen}
                    onThumbError={onThumbError}
                    selecting={selecting}
                    selected={selected.has(item.id)}
                    onSelect={onSelect}
                    onLongPress={onLongPress}
                    onDragOver={onDragOver}
                    onDragEnd={onDragEnd}
                    onFavorite={onFavorite}
                  />
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
