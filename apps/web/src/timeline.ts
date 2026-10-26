// The timeline's arithmetic, kept free of React so it can be tested: the library is described by per-day counts
// (GET /assets/timeline), and everything else follows from them. A photo's position in the whole library (its
// "global index", newest = 0) says which day it is in; the layout gives every month header, day header and row of
// tiles a fixed height, so the scrollbar and the scrubber are exact before any photo has been loaded.
import type { AssetListItem, TimelineDay } from "./api";

export const MONTH_HEADER_PX = 44;
export const DAY_HEADER_PX = 30;

export interface TimelineIndex {
  days: readonly TimelineDay[];
  /** Global index of each day's newest photo; `starts[i] + days[i].count` is where day i + 1 starts. */
  starts: readonly number[];
  total: number;
}

export function buildIndex(days: readonly TimelineDay[]): TimelineIndex {
  const kept = days.filter((d) => d.count > 0);
  const starts: number[] = [];
  let total = 0;
  for (const d of kept) {
    starts.push(total);
    total += d.count;
  }
  return { days: kept, starts, total };
}

/** The day (index into `days`) a global photo index falls in; -1 when out of range. */
export function dayOfIndex(idx: TimelineIndex, i: number): number {
  if (i < 0 || i >= idx.total) return -1;
  let lo = 0;
  let hi = idx.days.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (idx.starts[mid]! <= i) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** UTC calendar day (`YYYY-MM-DD`) of a timestamp, the same bucketing the server uses for the counts. */
export const dayOfTime = (iso: string): string => {
  const t = Date.parse(iso);
  return Number.isNaN(t) ? "" : new Date(t).toISOString().slice(0, 10);
};

/**
 * The `before` timestamp that makes the list start with `day`'s newest photo: the start of the next day
 * (the server returns photos strictly older).
 */
export function beforeDay(day: string): string {
  const [y = 0, m = 1, d = 1] = day.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d + 1)).toISOString();
}

const monthFmt = new Intl.DateTimeFormat("en-US", { month: "long", year: "numeric", timeZone: "UTC" });
const dayFmt = new Intl.DateTimeFormat("en-US", { weekday: "short", month: "short", day: "numeric", timeZone: "UTC" });
const utcNoon = (day: string) => new Date(`${day.slice(0, 10)}T12:00:00Z`);

/** "October 2026" for `2026-10` or any day of it. */
export const monthLabel = (monthOrDay: string): string => monthFmt.format(utcNoon(`${monthOrDay.slice(0, 7)}-01`));
/** "Tue, Oct 14" for `2026-10-14`. */
export const dayLabel = (day: string): string => dayFmt.format(utcNoon(day));
export const monthOf = (day: string) => day.slice(0, 7);
export const yearOf = (day: string) => day.slice(0, 4);

/** How the library's photos are laid out: square crops in equal columns, or rows that keep each photo's shape. */
export type GridStyle = "square" | "justified";

export interface Geometry {
  cols: number;
  /** Side of a (square) tile in px. */
  tile: number;
  gap: number;
  /** Absent means square. */
  style?: GridStyle;
  /** Width of the grid in px (justified rows are built to fill it exactly). */
  width?: number;
  /** The height justified rows aim for, in px. */
  rowH?: number;
}

export type Row =
  | { kind: "month"; key: string; top: number; height: number; month: string; label: string; count: number }
  | { kind: "day"; key: string; top: number; height: number; day: string; label: string }
  | {
      kind: "tiles";
      key: string;
      top: number;
      /** The row with the gap under it. */
      height: number;
      start: number;
      count: number;
      /** Justified rows: the aspect ratio of each photo, and the height of the tiles (without the gap). */
      ratios?: number[];
      tileH?: number;
      /** Justified rows: the photos fill the row's width (false for a short last row of a day, which keeps `rowH`). */
      fill?: boolean;
    };

export interface MonthMark {
  month: string;
  label: string;
  year: string;
  /** Offset of the month header from the top of the list. */
  top: number;
  /** Row index of the month header. */
  row: number;
  /** Global index of the month's newest photo, and how many photos it has. */
  first: number;
  count: number;
}

export interface Layout {
  rows: Row[];
  /** Indices (into `rows`) of the rows of tiles, in photo order. */
  tileRows: number[];
  height: number;
  months: MonthMark[];
  geometry: Geometry;
}

/** Aspect ratios outside this range are cropped to it in justified rows (a panorama would take the whole row). */
const MIN_RATIO = 0.5;
const MAX_RATIO = 2.4;
/** What a photo whose size is not known (yet) is assumed to be: 3:2. */
const DEFAULT_RATIO = 1.5;
/** A short last row of a day is stretched to fill the width when that makes its tiles at most this much taller than `rowH`. */
const MAX_STRETCH = 1.45;

/** A photo's shape for the justified rows: width / height, within limits; `fallback` where unknown. */
export function ratioOf(it: { width: number | null; height: number | null } | undefined, fallback = DEFAULT_RATIO): number {
  if (!it || !it.width || !it.height) return fallback;
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, it.width / it.height));
}

/**
 * Split `n` photos of one day into justified rows: add photos until a row is full, then take whichever of "with" or "without"
 * the last one makes the row's height closer to `rowH`. Each row is `[count, ratios, tileH, fill]`.
 */
export function justifiedRows(
  n: number,
  ratio: (k: number) => number,
  width: number,
  gap: number,
  rowH: number,
): { count: number; ratios: number[]; tileH: number; fill: boolean }[] {
  const out: { count: number; ratios: number[]; tileH: number; fill: boolean }[] = [];
  let k = 0;
  while (k < n) {
    const ratios: number[] = [];
    let sum = 0;
    let full = false;
    while (k + ratios.length < n) {
      const r = ratio(k + ratios.length);
      ratios.push(r);
      sum += r;
      if (sum * rowH + gap * (ratios.length - 1) >= width) {
        full = true;
        break;
      }
    }
    if (full && ratios.length > 1) {
      const m = ratios.length;
      const withLast = (width - gap * (m - 1)) / sum;
      const withoutLast = (width - gap * (m - 2)) / (sum - ratios[m - 1]!);
      if (Math.abs(withoutLast - rowH) < Math.abs(withLast - rowH)) {
        sum -= ratios.pop()!;
      }
    }
    const m = ratios.length;
    const filled = (width - gap * (m - 1)) / sum;
    if (full || filled <= rowH * MAX_STRETCH) {
      out.push({ count: m, ratios, tileH: filled, fill: true });
    } else {
      out.push({ count: m, ratios, tileH: rowH, fill: false });
    }
    k += m;
  }
  return out;
}

/**
 * Month header, then for each day a day header and its rows of tiles. Square rows have a fixed height, so the layout is exact
 * from the counts alone. Justified rows depend on the photos' shapes: those that are loaded (`loaded`) use theirs, the rest
 * the average of the loaded ones, and the heights settle as photos arrive.
 */
export function buildLayout(idx: TimelineIndex, g: Geometry, loaded?: Loaded): Layout {
  const rows: Row[] = [];
  const tileRows: number[] = [];
  const months: MonthMark[] = [];
  const justified = g.style === "justified" && !!g.width && !!g.rowH;
  const rowH = g.tile + g.gap;
  let fallback = DEFAULT_RATIO;
  if (justified && loaded && loaded.items.size > 0) {
    let sum = 0;
    for (const it of loaded.items.values()) sum += ratioOf(it);
    fallback = sum / loaded.items.size;
  }
  let top = 0;
  let month = "";
  for (let d = 0; d < idx.days.length; d++) {
    const { day, count } = idx.days[d]!;
    const m = monthOf(day);
    if (m !== month) {
      month = m;
      let monthCount = 0;
      for (let k = d; k < idx.days.length && monthOf(idx.days[k]!.day) === m; k++) monthCount += idx.days[k]!.count;
      const label = monthLabel(m);
      months.push({ month: m, label, year: yearOf(m), top, row: rows.length, first: idx.starts[d]!, count: monthCount });
      rows.push({ kind: "month", key: `m${m}`, top, height: MONTH_HEADER_PX, month: m, label, count: monthCount });
      top += MONTH_HEADER_PX;
    }
    rows.push({ kind: "day", key: `d${day}`, top, height: DAY_HEADER_PX, day, label: dayLabel(day) });
    top += DAY_HEADER_PX;
    const first = idx.starts[d]!;
    if (justified) {
      let at = first;
      for (const r of justifiedRows(count, (k) => ratioOf(loaded?.items.get(first + k), fallback), g.width!, g.gap, g.rowH!)) {
        const height = r.tileH + g.gap;
        tileRows.push(rows.length);
        rows.push({ kind: "tiles", key: `t${at}`, top, height, start: at, count: r.count, ratios: r.ratios, tileH: r.tileH, fill: r.fill });
        top += height;
        at += r.count;
      }
      continue;
    }
    for (let off = 0; off < count; off += g.cols) {
      tileRows.push(rows.length);
      rows.push({
        kind: "tiles",
        key: `t${first + off}`,
        top,
        height: rowH,
        start: first + off,
        count: Math.min(g.cols, count - off),
      });
      top += rowH;
    }
  }
  return { rows, tileRows, height: top, months, geometry: g };
}

/** The index (into `rows`) of the row of tiles that holds photo `photo`; -1 when there is none. */
export function rowOfPhoto(layout: Layout, photo: number): number {
  let lo = 0;
  let hi = layout.tileRows.length - 1;
  if (hi < 0) return -1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    const row = layout.rows[layout.tileRows[mid]!];
    if (row?.kind === "tiles" && row.start <= photo) lo = mid;
    else hi = mid - 1;
  }
  return layout.tileRows[lo]!;
}

/** Index of the row containing offset `y` (clamped). */
/**
 * Where to scroll (offset in the list) to show `day`: the top of its header, or of the nearest day with photos before it
 * (the library is newest first; a day with nothing in it has no header), or the last day when `day` is older than all.
 * Null for an empty library.
 */
export function dayTop(rows: readonly Row[], day: string): number | null {
  const days = rows.filter((r): r is Extract<Row, { kind: "day" }> => r.kind === "day");
  if (days.length === 0) return null;
  const hit = days.find((r) => r.day <= day) ?? days[days.length - 1]!;
  return hit.top;
}

export function rowAt(rows: readonly Row[], y: number): number {
  if (rows.length === 0) return -1;
  let lo = 0;
  let hi = rows.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (rows[mid]!.top <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** The month the given offset (the top of the viewport) is in. */
export function monthAt(layout: Layout, y: number): MonthMark | null {
  const { months } = layout;
  if (months.length === 0) return null;
  let lo = 0;
  let hi = months.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (months[mid]!.top <= y) lo = mid;
    else hi = mid - 1;
  }
  return months[lo]!;
}

/** Global photo indices covered by rows `[a, b]` (inclusive); null when they hold no photos. */
export function photoRange(layout: Layout, a: number, b: number): [number, number] | null {
  let from = -1;
  let to = -1;
  for (let r = Math.max(0, a); r <= Math.min(b, layout.rows.length - 1); r++) {
    const row = layout.rows[r]!;
    if (row.kind !== "tiles") continue;
    if (from < 0) from = row.start;
    to = row.start + row.count - 1;
  }
  return from < 0 ? null : [from, to];
}

/** Smallest and largest number of square columns the density setting can reach. */
export const MIN_COLS = 2;
export const MAX_COLS = 12;

/**
 * Columns, tile size and gap for a grid `width` px wide. Square: 3 columns on a phone, more as the screen widens; `density`
 * (a pinch) adds or removes columns. Justified: the height the rows aim for (a pinch scales it). The tiles always fill the
 * width exactly.
 */
export function gridGeometry(width: number, style: GridStyle = "square", density = 0): Geometry {
  const w = Math.max(1, Math.floor(width));
  const phone = w <= 600;
  const gap = phone ? 2 : 4;
  if (style === "justified") {
    const base = phone ? 118 : w >= 1100 ? 210 : 180;
    const rowH = Math.max(70, Math.round(base * Math.pow(0.8, density)));
    return { style, width: w, cols: Math.max(1, Math.round(w / rowH)), tile: rowH, gap, rowH };
  }
  const natural = phone ? (w >= 500 ? 4 : 3) : Math.max(4, Math.round((w + gap) / (170 + gap)));
  const cols = Math.min(MAX_COLS, Math.max(MIN_COLS, natural + density));
  return { style, width: w, cols, gap, tile: (w - gap * (cols - 1)) / cols };
}

// ---- Loaded photos -----------------------------------------------------------------------------------------

/** Photos that have been fetched, by global index, plus the keyset cursors that continue at an index. */
export interface Loaded {
  items: ReadonlyMap<number, AssetListItem>;
  /** `cursors.get(i)` is the cursor for the page that starts at global index `i`. */
  cursors: ReadonlyMap<number, string>;
}

export const emptyLoaded: Loaded = { items: new Map(), cursors: new Map() };

/** The first index in `[from, to]` (clamped to the library) that is not loaded. */
export function firstMissing(idx: TimelineIndex, loaded: Loaded, from: number, to: number): number | null {
  for (let i = Math.max(0, from); i <= Math.min(to, idx.total - 1); i++) {
    if (!loaded.items.has(i)) return i;
  }
  return null;
}

export interface PageRequest {
  /** The global index the response starts at. */
  base: number;
  cursor?: string;
  before?: string;
}

/**
 * How to fetch the page that contains index `i`: continue a known cursor if there is one, otherwise start at the top
 * of `i`'s day (`before` = the end of that day). A photo in the middle of a very large day is reached by walking
 * from the day's start, one page at a time.
 */
export function requestFor(idx: TimelineIndex, loaded: Loaded, i: number): PageRequest | null {
  const c = loaded.cursors.get(i);
  if (c) return { base: i, cursor: c };
  const d = dayOfIndex(idx, i);
  if (d < 0) return null;
  return { base: idx.starts[d]!, before: beforeDay(idx.days[d]!.day) };
}

/**
 * Put a page into `loaded`. Each photo must fall on the day the counts say it is on; if not (photos were added or
 * removed since the counts were read) nothing is stored and `stale` is true: the caller re-reads the counts.
 */
export function addPage(
  idx: TimelineIndex,
  loaded: Loaded,
  base: number,
  page: readonly AssetListItem[],
  nextCursor: string | null | undefined,
): { loaded: Loaded; stale: boolean } {
  for (let k = 0; k < page.length; k++) {
    const d = dayOfIndex(idx, base + k);
    if (d < 0 || idx.days[d]!.day !== dayOfTime(page[k]!.sort_time)) return { loaded, stale: true };
  }
  const items = new Map(loaded.items);
  const cursors = new Map(loaded.cursors);
  page.forEach((it, k) => items.set(base + k, it));
  if (nextCursor) cursors.set(base + page.length, nextCursor);
  return { loaded: { items, cursors }, stale: false };
}

/**
 * Forget loaded photos far from `center` once more than `max` are held, so memory stays flat however far the user
 * has scrolled. Photos whose ids are in `keep` (the selection) stay, and so does everything within `radius` of the
 * center. Returns `loaded` itself when nothing needs to go.
 */
export function evict(loaded: Loaded, center: number, max: number, radius: number, keep: ReadonlySet<string>): Loaded {
  if (loaded.items.size <= max) return loaded;
  const items = new Map<number, AssetListItem>();
  for (const [i, it] of loaded.items) {
    if (Math.abs(i - center) <= radius || keep.has(it.id)) items.set(i, it);
  }
  const cursors = new Map<number, string>();
  for (const [i, c] of loaded.cursors) if (Math.abs(i - center) <= radius) cursors.set(i, c);
  return { items, cursors };
}

/**
 * Remove photos by id (trashed, or taken out of a filtered view) and close the gaps: later photos move up, the day
 * counts shrink, and cursors (which point at old positions) are dropped. Unloaded photos keep their relative order
 * because only positions after each removed one shift.
 */
export function removeIds(
  idx: TimelineIndex,
  loaded: Loaded,
  ids: ReadonlySet<string>,
): { index: TimelineIndex; loaded: Loaded; removed: AssetListItem[] } {
  const gone: number[] = [];
  const removed: AssetListItem[] = [];
  for (const [i, it] of loaded.items) {
    if (ids.has(it.id)) {
      gone.push(i);
      removed.push(it);
    }
  }
  if (gone.length === 0) return { index: idx, loaded, removed };
  gone.sort((a, b) => a - b);
  const perDay = new Map<number, number>();
  for (const i of gone) {
    const d = dayOfIndex(idx, i);
    perDay.set(d, (perDay.get(d) ?? 0) + 1);
  }
  const days = idx.days.map((d, k) => ({ ...d, count: d.count - (perDay.get(k) ?? 0) }));
  const goneSet = new Set(gone);
  const items = new Map<number, AssetListItem>();
  let g = 0; // removed positions before the current one
  for (const i of [...loaded.items.keys()].sort((a, b) => a - b)) {
    while (g < gone.length && gone[g]! < i) g++;
    if (!goneSet.has(i)) items.set(i - g, loaded.items.get(i)!);
  }
  return { index: buildIndex(days), loaded: { items, cursors: new Map() }, removed };
}

/** The photos loaded around `index` as one unbroken run, at most `radius` each way (what the lightbox pages through). */
export function runAround(loaded: Loaded, index: number, radius: number): { start: number; items: AssetListItem[] } | null {
  if (!loaded.items.has(index)) return null;
  let a = index;
  while (a > index - radius && loaded.items.has(a - 1)) a--;
  let b = index;
  while (b < index + radius && loaded.items.has(b + 1)) b++;
  const items: AssetListItem[] = [];
  for (let i = a; i <= b; i++) items.push(loaded.items.get(i)!);
  return { start: a, items };
}

/** Loaded photos in timeline order. */
export const inOrder = (loaded: Loaded): AssetListItem[] =>
  [...loaded.items.entries()].sort((a, b) => a[0] - b[0]).map(([, it]) => it);

// ---- Scrubber ---------------------------------------------------------------------------------------------

export interface ScrubMark {
  month: string;
  label: string;
  /** Set on the first month of a year that has room for a printed year. */
  year: string | null;
  /** Position along the track, px from its top. */
  y: number;
}

/** 0..1 position of an offset along the scrollable range (`listHeight - viewport`). */
export const fractionOf = (offset: number, listHeight: number, viewport: number): number => {
  const range = listHeight - viewport;
  return range <= 0 ? 0 : Math.min(1, Math.max(0, offset / range));
};

/** The list offset that a 0..1 fraction of the track stands for. */
export const offsetOf = (fraction: number, listHeight: number, viewport: number): number =>
  Math.min(1, Math.max(0, fraction)) * Math.max(0, listHeight - viewport);

/**
 * Which months get a tick on a track `trackPx` tall (handle travel is `trackPx`): every month gets a tick unless it
 * would sit closer than `tickGap` to the previous one, and the first month of a year gets its year printed when
 * `labelGap` px are free above it. The marks are in top-to-bottom order.
 */
export function scrubMarks(
  layout: Layout,
  viewport: number,
  trackPx: number,
  { tickGap = 6, labelGap = 18 }: { tickGap?: number; labelGap?: number } = {},
): ScrubMark[] {
  const out: ScrubMark[] = [];
  let lastTick = -Infinity;
  let lastLabel = -Infinity;
  let lastYear = "";
  for (const m of layout.months) {
    const y = fractionOf(m.top, layout.height, viewport) * trackPx;
    const firstOfYear = m.year !== lastYear;
    lastYear = m.year;
    const showYear = firstOfYear && y - lastLabel >= labelGap;
    if (showYear) {
      lastLabel = y;
      lastTick = y;
      out.push({ month: m.month, label: m.label, year: m.year, y });
    } else if (y - lastTick >= tickGap && y - lastLabel >= 10) {
      lastTick = y;
      out.push({ month: m.month, label: m.label, year: null, y });
    }
  }
  return out;
}
