import { useWindowVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AssetListItem } from "../api";
import { buildLayout, gridGeometry, photoRange, type Loaded, type TimelineIndex } from "../timeline";
import { Scrubber } from "./Scrubber";
import { Tile } from "./Tile";

/** Rows beyond the viewport that are rendered, each way. */
const OVERSCAN_ROWS = 3;
/** Photos past the last visible one to start fetching, so scrolling down rarely meets an empty tile. */
const PREFETCH = 60;

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
  onFavorite: (id: string, favorite: boolean) => void;
}

/**
 * The library as date-grouped rows (month header, day header, rows of square tiles) of which only the ones near the
 * viewport exist in the DOM. Row heights are fixed, so the layout of the whole library (and with it the scrollbar and
 * the scrubber) is exact from the per-day counts alone; tiles whose photos are not loaded yet are placeholders.
 * The page itself scrolls (window virtualizer), so the sticky bars and the browser's scroll restoration just work.
 */
export function TimelineGrid({ index, loaded, onRange, onOpen, onThumbError, selecting, selected, onSelect, onLongPress, onFavorite }: Props) {
  const listRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(() => (typeof window === "undefined" ? 1024 : window.innerWidth) - 26);
  const [scrollMargin, setScrollMargin] = useState(0);
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
    // Things above the list (the selection bar, notices) move it without resizing it.
    const above = el.closest(".library");
    if (above) ro?.observe(above);
    window.addEventListener("resize", measure);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, []);
  const layout = useMemo(() => buildLayout(index, gridGeometry(width)), [index, width]);
  const { rows } = layout;

  const virtualizer = useWindowVirtualizer({
    count: rows.length,
    estimateSize: (i) => rows[i]!.height,
    getItemKey: (i) => rows[i]!.key,
    overscan: OVERSCAN_ROWS,
    scrollMargin,
  });
  // Sizes are exact and never measured; when the layout changes forget the cached ones.
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [layout, virtualizer]);

  const virtual = virtualizer.getVirtualItems();
  const first = virtual[0]?.index ?? -1;
  const last = virtual.at(-1)?.index ?? -1;
  useEffect(() => {
    if (first < 0 || scrubbing) return;
    const r = photoRange(layout, first, last);
    if (r) onRange(r[0], r[1] + PREFETCH);
  }, [first, last, layout, onRange, scrubbing]);

  const { cols, gap } = layout.geometry;
  const grid = { gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, gap } as const;

  return (
    <div className="timeline">
      {/* First, so the floating rail sits at the top of the timeline and sticks from there. */}
      <Scrubber layout={layout} scrollMargin={scrollMargin} onDragging={setScrubbing} />
      <div className="tl-list" ref={listRef} style={{ height: virtualizer.getTotalSize() }} role="list" aria-label="Photos">
        {virtual.map((v) => {
          const row = rows[v.index];
          if (!row) return null;
          const style = { height: row.height, transform: `translateY(${v.start - scrollMargin}px)` };
          if (row.kind === "month") {
            return (
              <div key={v.key} className="tl-row tl-month" style={style} role="listitem">
                <h2>{row.label}</h2>
              </div>
            );
          }
          if (row.kind === "day") {
            return (
              <div key={v.key} className="tl-row tl-day" style={style} role="listitem">
                <h3>{row.label}</h3>
              </div>
            );
          }
          return (
            <div key={v.key} className="tl-row tl-tiles" style={{ ...style, ...grid, paddingBottom: gap }} role="listitem">
              {Array.from({ length: row.count }, (_, k) => {
                const i = row.start + k;
                const item: AssetListItem | undefined = loaded.items.get(i);
                if (!item) return <div key={`p${i}`} className="tile-cell tile-skeleton" aria-hidden="true" />;
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
