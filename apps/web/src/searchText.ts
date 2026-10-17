// Pure helpers of the search UI: the URL <-> filters mapping, the labels of the filter chips, the indexing message and
// the recent-searches list. Kept apart from the components so the rules are unit tested.
import type { Processing, SearchParams } from "./api";

const TYPES = ["jpeg", "heic", "raw", "other"] as const;
export const TYPE_LABEL: Record<(typeof TYPES)[number], string> = {
  jpeg: "JPEG",
  heic: "HEIC",
  raw: "RAW",
  other: "Other",
};

const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** `/search?...` query string to params. Unknown or malformed values are dropped, never passed to the server. */
export function paramsFromUrl(search: URLSearchParams): SearchParams {
  const out: SearchParams = {};
  const q = search.get("q")?.trim();
  if (q) out.q = q.slice(0, 200);
  for (const k of ["from", "to"] as const) {
    const v = search.get(k);
    if (v && DAY.test(v)) out[k] = v;
  }
  if (search.get("favorite") === "true") out.favorite = true;
  const camera = search.get("camera")?.trim();
  if (camera) out.camera = camera;
  const place = search.get("place")?.trim();
  if (place) out.place = place;
  const type = search.get("type");
  if (type && (TYPES as readonly string[]).includes(type)) out.type = type as SearchParams["type"];
  const album = search.get("album");
  if (album && /^[0-9a-f-]{36}$/i.test(album)) out.album = album;
  const loc = search.get("has_location");
  if (loc === "true") out.has_location = true;
  if (loc === "false") out.has_location = false;
  return out;
}

/** Params to the `/search` URL's query string (the same names as the API). */
export function urlFromParams(p: SearchParams): string {
  const q = new URLSearchParams();
  if (p.q) q.set("q", p.q);
  if (p.from) q.set("from", p.from);
  if (p.to) q.set("to", p.to);
  if (p.favorite) q.set("favorite", "true");
  if (p.camera) q.set("camera", p.camera);
  if (p.place) q.set("place", p.place);
  if (p.type) q.set("type", p.type);
  if (p.album) q.set("album", p.album);
  if (p.has_location !== undefined) q.set("has_location", String(p.has_location));
  return q.toString();
}

/** Is there anything to search for: a query or at least one filter. */
export const isEmptySearch = (p: SearchParams) => Object.keys(p).length === 0;

/** Does the search have a filter (a query alone is not one)? */
export const hasFilters = (p: SearchParams) => Object.keys(p).some((k) => k !== "q");

export interface Chip {
  /** Which parameters clearing the chip removes. */
  clears: (keyof SearchParams)[];
  label: string;
}

const fmtDay = (d: string) => {
  const [y, m, day] = d.split("-").map(Number);
  return new Date(Date.UTC(y!, m! - 1, day!)).toLocaleDateString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
};

/** "2024", "Mar 2024" or "Mar 1, 2024 to Mar 9, 2024": the chip for a date range. */
export function dateRangeLabel(from?: string, to?: string): string {
  if (from && to) {
    if (from.slice(0, 4) === to.slice(0, 4) && from.slice(5) === "01-01" && to.slice(5) === "12-31") return from.slice(0, 4);
    if (from === to) return fmtDay(from);
    return `${fmtDay(from)} to ${fmtDay(to)}`;
  }
  if (from) return `From ${fmtDay(from)}`;
  if (to) return `Until ${fmtDay(to)}`;
  return "";
}

/** The chips under the search bar, one per active filter. `albumTitle` names the album filter (an id alone is no label). */
export function chipsOf(p: SearchParams, albumTitle?: string): Chip[] {
  const chips: Chip[] = [];
  if (p.from || p.to) chips.push({ clears: ["from", "to"], label: dateRangeLabel(p.from, p.to) });
  if (p.favorite) chips.push({ clears: ["favorite"], label: "Favorites" });
  if (p.camera) chips.push({ clears: ["camera"], label: p.camera });
  if (p.place) chips.push({ clears: ["place"], label: p.place });
  if (p.type) chips.push({ clears: ["type"], label: TYPE_LABEL[p.type] });
  if (p.has_location !== undefined) chips.push({ clears: ["has_location"], label: p.has_location ? "Has location" : "No location" });
  if (p.album) chips.push({ clears: ["album"], label: albumTitle ? `Album: ${albumTitle}` : "Album" });
  return chips;
}

/** Whole calendar year `y` as the `from`/`to` pair of the year facet. */
export const yearRange = (y: string) => ({ from: `${y}-01-01`, to: `${y}-12-31` });

const n = (v: number) => v.toLocaleString("en-US");

/**
 * "Still indexing 1,234 photos: results may be incomplete" while the worker is embedding the library; null when search
 * covers every photo, or the server does not report it. `embedding.total - indexed` can be a few off while photos are
 * being added; only a queue that is actually working counts as indexing.
 */
export function indexingNote(p: Processing | null): string | null {
  const e = p?.embedding;
  if (!e || e.total <= e.indexed) return null;
  const left = e.total - e.indexed;
  if (e.queued + e.running === 0) {
    // Nothing queued yet photos lack a vector: thumbnails come first, or the model is not installed.
    return p!.without_thumbnail > 0 ? null : `${n(left)} ${left === 1 ? "photo is" : "photos are"} not searchable yet.`;
  }
  return `Still indexing ${n(left)} ${left === 1 ? "photo" : "photos"}: results may be incomplete.`;
}

/** 0..100 of the library that search covers, or null when everything is indexed or nothing is known. */
export function indexingPercent(p: Processing | null): number | null {
  const e = p?.embedding;
  if (!e || e.total <= 0 || e.indexed >= e.total) return null;
  return Math.max(0, Math.min(99, Math.floor((e.indexed / e.total) * 100)));
}

// ---- recent searches (this browser only) ------------------------------------------------------------------------

const RECENT_KEY = "cyanotype.recentSearches";
export const RECENT_MAX = 8;

export function loadRecent(store: Pick<Storage, "getItem"> = localStorage): string[] {
  try {
    const v: unknown = JSON.parse(store.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, RECENT_MAX) : [];
  } catch {
    return [];
  }
}

/** Put `q` first (once, case-insensitively), keep the newest [`RECENT_MAX`]. Returns the new list. */
export function pushRecent(q: string, store: Pick<Storage, "getItem" | "setItem"> = localStorage): string[] {
  const text = q.trim();
  if (!text) return loadRecent(store);
  const next = [text, ...loadRecent(store).filter((r) => r.toLowerCase() !== text.toLowerCase())].slice(0, RECENT_MAX);
  try {
    store.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    /* private mode or full: the list just does not persist */
  }
  return next;
}

export function clearRecent(store: Pick<Storage, "removeItem"> = localStorage) {
  try {
    store.removeItem(RECENT_KEY);
  } catch {
    /* nothing to clear */
  }
}

/** Recent searches that start with what is typed (all of them when it is empty), never the exact text itself. */
export function suggestionsFor(typed: string, recent: readonly string[]): string[] {
  const t = typed.trim().toLowerCase();
  return recent.filter((r) => r.toLowerCase().startsWith(t) && r.toLowerCase() !== t);
}
