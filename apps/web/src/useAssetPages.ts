import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, type AssetList, type AssetListItem } from "./api";
import { useAuth } from "./auth";

/**
 * Keyset-paged photo list with infinite scroll: attach `sentinel` to an element after the grid.
 * `fetchPage` must be stable (useCallback) and return `next_cursor` null on the last page.
 */
export function useAssetPages(fetchPage: (cursor: string | null) => Promise<AssetList>) {
  const { setAnonymous } = useAuth();
  const [items, setItems] = useState<AssetListItem[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [started, setStarted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const inFlight = useRef(false);
  const sentinel = useRef<HTMLDivElement>(null);

  const loadMore = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      const page = await fetchPage(cursor);
      setItems((cur) => {
        const seen = new Set(cur.map((i) => i.id));
        return [...cur, ...page.items.filter((i) => !seen.has(i.id))];
      });
      setCursor(page.next_cursor ?? null);
      setDone(!page.next_cursor);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not load the photos.");
    } finally {
      inFlight.current = false;
      setLoading(false);
      setStarted(true);
    }
  }, [cursor, fetchPage, setAnonymous]);

  // First page, on mount and again after `reset`.
  useEffect(() => {
    void loadMore();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generation]);

  /** Forget everything loaded and start again from the newest photo. */
  const reset = useCallback(() => {
    setItems([]);
    setCursor(null);
    setDone(false);
    setStarted(false);
    setGeneration((g) => g + 1);
  }, []);

  useEffect(() => {
    const el = sentinel.current;
    if (!el || !started || done || error) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) void loadMore();
      },
      { rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [started, done, error, items.length, loadMore]);

  const refreshItem = useCallback(
    (id: string, patch: Partial<AssetListItem>) =>
      setItems((cur) => cur.map((i) => (i.id === id ? { ...i, ...patch } : i))),
    [],
  );

  return { items, setItems, done, started, loading, error, loadMore, sentinel, refreshItem, reset };
}
