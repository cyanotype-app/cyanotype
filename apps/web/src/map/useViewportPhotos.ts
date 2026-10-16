import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type AssetListItem } from "../api";

/** Photos fetched per page of the viewport list. */
export const VIEWPORT_PAGE = 60;

/**
 * The photos inside `bbox` (`west,south,east,north`, or null for none yet), newest first, keyset-paged: the first page
 * loads when the box changes, `loadMore` fetches the next. A box that changes while a page is in flight discards that
 * page, so a quick pan never mixes two areas.
 */
export function useViewportPhotos(bbox: string | null, onUnauthorized: () => void) {
  const [items, setItems] = useState<AssetListItem[]>([]);
  const [done, setDone] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useRef(0); // identifies the current box
  const cursor = useRef<string | null>(null);
  const busy = useRef(false);
  const finished = useRef(false);
  const box = useRef<string | null>(bbox);
  const unauthorized = useRef(onUnauthorized);
  useEffect(() => {
    unauthorized.current = onUnauthorized;
  });

  const fetchPage = useCallback(async () => {
    const mine = run.current;
    const area = box.current;
    if (!area || busy.current || finished.current) return;
    busy.current = true;
    setLoading(true);
    setError(null);
    try {
      const page = await api.listAssets({ bbox: area, cursor: cursor.current, limit: VIEWPORT_PAGE });
      if (mine !== run.current) return;
      cursor.current = page.next_cursor ?? null;
      finished.current = !page.next_cursor;
      setDone(finished.current);
      setItems((cur) => {
        const seen = new Set(cur.map((i) => i.id));
        return [...cur, ...page.items.filter((i) => !seen.has(i.id))];
      });
    } catch (e) {
      if (mine !== run.current) return;
      if (e instanceof ApiError && e.status === 401) unauthorized.current();
      else setError(e instanceof Error ? e.message : "Could not load the photos.");
    } finally {
      if (mine === run.current) {
        busy.current = false;
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    run.current += 1;
    box.current = bbox;
    cursor.current = null;
    busy.current = false;
    finished.current = false;
    setItems([]);
    setDone(false);
    setError(null);
    setLoading(false);
    if (bbox) void fetchPage();
  }, [bbox, fetchPage]);

  /** Change a loaded photo in place (a favorite, a refreshed thumbnail). */
  const patch = useCallback(
    (id: string, change: Partial<AssetListItem>) =>
      setItems((cur) => cur.map((i) => (i.id === id ? { ...i, ...change } : i))),
    [],
  );
  const remove = useCallback((id: string) => setItems((cur) => cur.filter((i) => i.id !== id)), []);

  return { items, done, loading, error, loadMore: fetchPage, patch, remove };
}
