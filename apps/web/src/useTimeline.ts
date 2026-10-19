import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ApiError, api, type AssetListItem, type LibraryView } from "./api";
import { useAuth } from "./auth";
import {
  addPage,
  buildIndex,
  emptyLoaded,
  evict,
  firstMissing,
  removeIds as removeFromLoaded,
  requestFor,
  type Loaded,
  type TimelineIndex,
} from "./timeline";

/** Photos fetched per request. */
export const PAGE = 120;
/** Hold at most this many loaded photos; the ones far from the viewport are dropped (and fetched again if needed). */
const MAX_LOADED = 2400;
const KEEP_RADIUS = 1000;
const DEBOUNCE_MS = 80;
/** Stale count reads in a row before giving up (photos added or removed while loading shift the counts). */
const MAX_STALE = 3;

export interface TimelineState {
  status: "loading" | "ready" | "error";
  error: string | null;
  index: TimelineIndex;
  loaded: Loaded;
}

const initial: TimelineState = { status: "loading", error: null, index: buildIndex([]), loaded: emptyLoaded };

/**
 * The library as a timeline: per-day counts first (so the whole layout is known), then photos on demand for whatever
 * range the view reports with `ensure`. Pages are keyed by global index, so a jump to any date loads just the page
 * there. `keep` returns ids that must not be evicted (the selection).
 */
export function useTimeline({ view, keep }: { view: LibraryView; keep: () => ReadonlySet<string> }) {
  // A view is a handful of flags: key the callbacks on their values, not on the object a caller makes anew each render.
  const { favorite, archived, hidden, sort } = view;
  const v = useMemo<LibraryView>(() => ({ favorite, archived, hidden, sort }), [favorite, archived, hidden, sort]);
  const { setAnonymous } = useAuth();
  const [state, setState] = useState<TimelineState>(initial);
  const stateRef = useRef(state);
  const gen = useRef(0); // bumped whenever in-flight page requests must be discarded
  const range = useRef({ from: 0, to: PAGE - 1 });
  const inFlight = useRef(false);
  const stale = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const keepRef = useRef(keep);
  useEffect(() => {
    keepRef.current = keep;
  });
  const pumpRef = useRef<() => void>(() => undefined);

  const commit = useCallback((next: TimelineState) => {
    stateRef.current = next;
    setState(next);
  }, []);

  const fail = useCallback(
    (e: unknown, fallback: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else if (e instanceof ApiError && e.code === "hidden_locked") commit({ ...stateRef.current, status: "error", error: "hidden_locked" });
      else commit({ ...stateRef.current, status: stateRef.current.status === "ready" ? "ready" : "error", error: e instanceof Error ? e.message : fallback });
    },
    [setAnonymous, commit],
  );

  /** Read the counts and start over (nothing loaded). */
  const load = useCallback(async () => {
    gen.current++;
    try {
      const t = await api.timeline(v);
      stale.current = 0;
      commit({ status: "ready", error: null, index: buildIndex(t.days), loaded: emptyLoaded });
    } catch (e) {
      fail(e, "Could not load your photos.");
    }
  }, [v, commit, fail]);

  const pump = useCallback(async () => {
    if (inFlight.current) return;
    const st = stateRef.current;
    if (st.status !== "ready" || st.error) return;
    const missing = firstMissing(st.index, st.loaded, range.current.from, range.current.to);
    if (missing === null) return;
    const req = requestFor(st.index, st.loaded, missing);
    if (!req) return;
    inFlight.current = true;
    const myGen = gen.current;
    let again = true;
    try {
      const page = await api.listAssets({ cursor: req.cursor, before: req.before, limit: PAGE, ...v });
      if (myGen !== gen.current) return;
      const cur = stateRef.current;
      if (cur.index !== st.index) return; // counts changed under us (a removal): the next pump starts clean
      const added = page.items.length > 0 ? addPage(cur.index, cur.loaded, req.base, page.items, page.next_cursor) : null;
      if (!added || added.stale) {
        // The library changed since the counts were read. Read them again and start over.
        again = false;
        if (++stale.current > MAX_STALE) {
          commit({ ...cur, error: "The library keeps changing; reload to continue." });
        } else {
          await load();
          again = true;
        }
        return;
      }
      stale.current = 0;
      const center = Math.round((range.current.from + range.current.to) / 2);
      commit({ ...cur, loaded: evict(added.loaded, center, MAX_LOADED, KEEP_RADIUS, keepRef.current()) });
    } catch (e) {
      again = false;
      fail(e, "Could not load your photos.");
    } finally {
      inFlight.current = false;
      if (again) pumpRef.current();
    }
  }, [v, load, commit, fail]);

  useEffect(() => {
    pumpRef.current = () => void pump();
  }, [pump]);

  const retry = useCallback(() => {
    const st = stateRef.current;
    if (st.status !== "ready") {
      commit({ ...initial });
      void load();
    } else {
      commit({ ...st, error: null });
      void pump();
    }
  }, [commit, load, pump]);

  useEffect(() => {
    void load();
    return () => clearTimeout(timer.current);
  }, [load]);

  // When the counts arrive, load what the view already asked for.
  const ready = state.status === "ready";
  const indexRef = state.index;
  useEffect(() => {
    if (ready) void pump();
  }, [ready, indexRef, pump]);

  /** The view shows (or is about to show) photos `from..to`: fetch whatever of that is missing (after a short pause
   * while scrolling, at once for `immediate`). */
  const ensure = useCallback(
    (from: number, to: number, immediate = false) => {
      range.current = { from, to };
      clearTimeout(timer.current);
      if (immediate) void pump();
      else timer.current = setTimeout(() => void pump(), DEBOUNCE_MS);
    },
    [pump],
  );

  /**
   * Re-read the counts and the page at the current position, then swap both in at once: nothing flickers, and
   * photos added or restored elsewhere show up. Failures keep what is on screen.
   */
  const refresh = useCallback(async () => {
    const myGen = ++gen.current;
    for (let attempt = 0; attempt < MAX_STALE; attempt++) {
      try {
        const t = await api.timeline(v);
        const index = buildIndex(t.days);
        const at = Math.min(Math.max(0, range.current.from), Math.max(0, index.total - 1));
        let loaded = emptyLoaded;
        if (index.total > 0) {
          const req = requestFor(index, emptyLoaded, at);
          if (!req) return;
          const page = await api.listAssets({ before: req.before, limit: PAGE, ...v });
          const added = addPage(index, emptyLoaded, req.base, page.items, page.next_cursor);
          if (added.stale || page.items.length === 0) continue;
          loaded = added.loaded;
        }
        if (myGen !== gen.current) return;
        gen.current++;
        commit({ status: "ready", error: null, index, loaded });
        return;
      } catch {
        return; // keep what is shown; the next change will try again
      }
    }
  }, [v, commit]);

  /** Drop photos from the view at once (trashed, or no longer favorites) and close the gaps. Returns what was removed. */
  const remove = useCallback(
    (ids: ReadonlySet<string>): AssetListItem[] => {
      const cur = stateRef.current;
      const r = removeFromLoaded(cur.index, cur.loaded, ids);
      if (r.removed.length === 0) return [];
      gen.current++;
      commit({ ...cur, index: r.index, loaded: r.loaded });
      return r.removed;
    },
    [commit],
  );

  /** Change fields of loaded photos in place. */
  const patch = useCallback(
    (ids: ReadonlySet<string>, fields: Partial<AssetListItem>) => {
      const cur = stateRef.current;
      let changed = false;
      const items = new Map(cur.loaded.items);
      for (const [i, it] of cur.loaded.items) {
        if (ids.has(it.id)) {
          items.set(i, { ...it, ...fields });
          changed = true;
        }
      }
      if (changed) commit({ ...cur, loaded: { ...cur.loaded, items } });
    },
    [commit],
  );

  return { ...state, ensure, refresh, remove, patch, retry };
}
