import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type AssetListItem } from "../api";
import { useAuth } from "../auth";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";

const PAGE = 60;

export function Library() {
  const { setAnonymous } = useAuth();
  const [items, setItems] = useState<AssetListItem[]>([]);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [done, setDone] = useState(false); // the last page has been loaded
  const [started, setStarted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const inFlight = useRef(false);
  const sentinel = useRef<HTMLDivElement>(null);

  const loadMore = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      const page = await api.listAssets({ before: nextBefore, limit: PAGE });
      setItems((cur) => {
        const seen = new Set(cur.map((i) => i.id));
        return [...cur, ...page.items.filter((i) => !seen.has(i.id))];
      });
      setNextBefore(page.next_before);
      setDone(page.next_before === null);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not load your photos.");
    } finally {
      inFlight.current = false;
      setLoading(false);
      setStarted(true);
    }
  }, [nextBefore, setAnonymous]);

  // First page.
  useEffect(() => {
    void loadMore(); // first page, on mount only
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Infinite scroll: whenever the sentinel is (still) visible and there is more, load the next page.
  // Re-created after every page so a sentinel that stays on screen keeps loading.
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

  const refreshItem = useCallback(async (id: string) => {
    try {
      const d = await api.getAsset(id);
      setItems((cur) => cur.map((i) => (i.id === id ? { ...i, thumb_url: d.thumb_url, thumbhash: d.thumbhash } : i)));
    } catch {
      /* leave the tile as is */
    }
  }, []);

  const closeLightbox = useCallback(() => setOpen(null), []);

  // Opening or paging to one of the last loaded photos pulls in the next page so ‹ › keeps going.
  const select = useCallback(
    (i: number) => {
      setOpen(i);
      if (i >= items.length - 3 && !done && !error) void loadMore();
    },
    [items.length, done, error, loadMore],
  );

  if (!started) return <p className="center muted" role="status">Loading your photos…</p>;

  if (error && items.length === 0) {
    return (
      <div className="empty" role="alert">
        <h1>Couldn’t load your library</h1>
        <p className="muted">{error}</p>
        <button type="button" className="btn btn-primary" onClick={() => void loadMore()}>
          Try again
        </button>
      </div>
    );
  }

  if (items.length === 0) {
    return (
      <div className="empty">
        <h1>Your library is empty</h1>
        <p>
          Photos are uploaded with the command-line tool for now. Browser uploads are coming once direct-to-storage
          CORS is sorted out.
        </p>
        <p className="muted">After an upload finishes processing, reload this page and your photos will appear here.</p>
      </div>
    );
  }

  return (
    <>
      <h1 className="sr-only">Library</h1>
      <Grid items={items} onOpen={select} onThumbError={(id) => void refreshItem(id)} />
      <div ref={sentinel} className="sentinel" aria-hidden="true" />
      <p className="muted center small" role="status">
        {loading ? "Loading more…" : done ? `${items.length} photo${items.length === 1 ? "" : "s"}` : ""}
      </p>
      {error && (
        <p className="center" role="alert">
          {error}{" "}
          <button type="button" className="btn btn-quiet" onClick={() => void loadMore()}>
            Retry
          </button>
        </p>
      )}
      {open !== null && (
        <Lightbox
          items={items}
          index={Math.min(open, items.length - 1)}
          onIndexChange={select}
          onClose={closeLightbox}
          loadDetail={api.getAsset}
        />
      )}
    </>
  );
}
