import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError, type AssetListItem, type MapCluster, type TilesInstalled } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { Grid } from "../components/Grid";
import { LibraryTabs } from "../components/LibraryTabs";
import { Lightbox } from "../components/Lightbox";
import { useToast } from "../components/Toast";
import { bboxParam, clusterZoom, extent, WORLD, type Bounds } from "../map/geo";
import type { MapViewport } from "../map/MapCanvas";
import { useViewportPhotos } from "../map/useViewportPhotos";

// maplibre-gl and pmtiles are big; they load only when the map page opens.
const MapCanvas = lazy(() => import("../map/MapCanvas"));

/** The view is acted on this long after the user stops moving it. */
const SETTLE_MS = 200;
const darkQuery = () => window.matchMedia?.("(prefers-color-scheme: dark)");

const photos = (n: number) => `${n.toLocaleString()} photo${n === 1 ? "" : "s"}`;

/** Where the geotagged photos are: a clustered map and, beside or under it, the photos in the part on screen. */
export function MapPage() {
  const { setAnonymous } = useAuth();
  const { toast, show } = useToast();
  const [tiles, setTiles] = useState<TilesInstalled | null>(null);
  const [overview, setOverview] = useState<{ total: number; fit: Bounds | null } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [view, setView] = useState<MapViewport | null>(null);
  const [clusters, setClusters] = useState<MapCluster[]>([]);
  const [inView, setInView] = useState<{ total: number; truncated: boolean } | null>(null);
  const [mapError, setMapError] = useState<string | null>(null);
  const [dark, setDark] = useState(() => darkQuery()?.matches ?? false);
  const [lightbox, setLightbox] = useState<{ items: AssetListItem[]; index: number } | null>(null);
  const [addIds, setAddIds] = useState<string[] | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);

  const unauthorized = useCallback(() => setAnonymous(), [setAnonymous]);

  // Which basemap is installed, and where the photos are overall (the first view and the empty state).
  useEffect(() => {
    let live = true;
    const fail = (e: unknown) => {
      if (!live) return;
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setLoadError(e instanceof Error ? e.message : "Could not load the map.");
    };
    api
      .tiles()
      .then((t) => live && setTiles(t))
      .catch(() => live && setTiles({ world: false, detail: false })); // no basemap: the plain-background fallback
    api
      .mapClusters(bboxParam(WORLD), 0)
      .then((r) => live && setOverview({ total: r.total, fit: extent(r.clusters) }))
      .catch(fail);
    return () => {
      live = false;
    };
  }, [setAnonymous]);

  useEffect(() => {
    const q = darkQuery();
    if (!q) return;
    const on = () => setDark(q.matches);
    q.addEventListener("change", on);
    return () => q.removeEventListener("change", on);
  }, []);

  // The view the list and the markers describe: settled, so a drag does not fire a request per frame.
  const [settled, setSettled] = useState<MapViewport | null>(null);
  useEffect(() => {
    if (!view) return;
    const t = setTimeout(() => setSettled(view), SETTLE_MS);
    return () => clearTimeout(t);
  }, [view]);

  // Without a working map the list shows the whole library's geotagged photos.
  const effective: MapViewport | null = mapError ? { bounds: WORLD, zoom: 0 } : settled;
  const bbox = effective ? bboxParam(effective.bounds) : null;
  const zoom = effective ? clusterZoom(effective.zoom) : 0;

  useEffect(() => {
    if (!bbox) return;
    const ctl = new AbortController();
    api
      .mapClusters(bbox, zoom, ctl.signal)
      .then((r) => {
        if (ctl.signal.aborted) return;
        setClusters(r.clusters);
        setInView({ total: r.total, truncated: r.truncated });
      })
      .catch((e: unknown) => {
        if (ctl.signal.aborted) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        // A failed refresh leaves the markers as they were.
      });
    return () => ctl.abort();
  }, [bbox, zoom, refreshTick, setAnonymous]);

  const list = useViewportPhotos(bbox, unauthorized);
  const { items, patch, remove } = list;

  // Reset the list's scroll when the area changes, and fetch the next page as its end comes into view.
  const listEl = useRef<HTMLDivElement>(null);
  const sentinel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (listEl.current) listEl.current.scrollTop = 0;
  }, [bbox]);
  const { loadMore, done, loading, error: listError } = list;
  useEffect(() => {
    const el = sentinel.current;
    if (!el || done || loading || listError) return;
    const io = new IntersectionObserver((e) => e.some((x) => x.isIntersecting) && void loadMore(), {
      root: listEl.current,
      rootMargin: "400px 0px",
    });
    io.observe(el);
    return () => io.disconnect();
  }, [done, loading, listError, loadMore, items.length]);

  const refreshThumb = useCallback(
    (id: string) =>
      void api.getAsset(id).then(
        (d) => patch(id, { thumb_url: d.thumb_url, thumbhash: d.thumbhash }),
        () => undefined,
      ),
    [patch],
  );

  // A marker opens its photo; the lightbox pages through the list when the photo is in it, else it shows just that one.
  const openPhoto = useCallback(
    async (id: string) => {
      const at = items.findIndex((i) => i.id === id);
      if (at >= 0) return setLightbox({ items, index: at });
      try {
        const d = await api.getAsset(id);
        setLightbox({ items: [d], index: 0 });
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else show({ tone: "error", message: "Couldn’t open that photo." });
      }
    },
    [items, setAnonymous, show],
  );

  const patchBoth = useCallback(
    (id: string, change: Partial<AssetListItem>) => {
      patch(id, change);
      setLightbox((lb) => lb && { ...lb, items: lb.items.map((i) => (i.id === id ? { ...i, ...change } : i)) });
    },
    [patch],
  );

  const setFavorite = useCallback(
    async (it: AssetListItem, favorite: boolean) => {
      patchBoth(it.id, { favorite });
      try {
        await api.favoriteAssets([it.id], favorite);
      } catch (e) {
        patchBoth(it.id, { favorite: !favorite });
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else show({ tone: "error", message: "Couldn’t update the favorite." });
      }
    },
    [patchBoth, setAnonymous, show],
  );

  const trash = useCallback(
    async (it: AssetListItem) => {
      remove(it.id);
      setLightbox((lb) => {
        if (!lb) return lb;
        const rest = lb.items.filter((i) => i.id !== it.id);
        return rest.length === 0 ? null : { items: rest, index: Math.min(lb.index, rest.length - 1) };
      });
      try {
        await api.trashAssets([it.id]);
      } catch (e) {
        setRefreshTick((n) => n + 1);
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else show({ tone: "error", message: "Couldn’t move to trash." });
        return;
      }
      setRefreshTick((n) => n + 1);
      show({
        message: "1 moved to trash",
        action: {
          label: "Undo",
          onClick: () =>
            void api.restoreAssets([it.id]).then(
              () => {
                setRefreshTick((n) => n + 1);
                show({ message: "1 restored" });
              },
              () => show({ tone: "error", message: "Couldn’t undo. It is still in the trash." }),
            ),
        },
      });
    },
    [remove, setAnonymous, show],
  );

  if (loadError) {
    return (
      <div className="library map-page">
        <LibraryTabs />
        <div className="empty" role="alert">
          <h1>Couldn’t load the map</h1>
          <p className="muted">{loadError}</p>
        </div>
      </div>
    );
  }
  if (!overview || !tiles) {
    return (
      <div className="library map-page">
        <LibraryTabs />
        <p className="center muted" role="status">
          Loading the map…
        </p>
      </div>
    );
  }
  const basemap = tiles;
  if (overview.total === 0) {
    return (
      <div className="library map-page">
        <LibraryTabs />
        <div className="empty">
          <h1>No photos with a location yet</h1>
          <p>
            Photos that carry a GPS position in their EXIF data show up here once they are processed. Phone photos
            usually do; most camera files do not.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="library map-page">
      <LibraryTabs />
      <h1 className="sr-only">Map</h1>
      {!basemap.world && !mapError && (
        <p className="map-notice muted small" role="note">
          No basemap is installed on this server, so photos are drawn on a plain background. The admin can add one
          (see “Map” in the deploy guide).
        </p>
      )}
      {mapError && (
        <p className="map-notice small" role="alert">
          The map can’t be drawn in this browser ({mapError}). The photos with a location are listed below.
        </p>
      )}
      <div className="map-layout">
        {!mapError && (
          <section className="map-pane" aria-label="Map of your photos">
            <Suspense
              fallback={
                <p className="map-loading muted" role="status">
                  Loading the map…
                </p>
              }
            >
              <MapCanvas
                clusters={clusters}
                basemap={basemap}
                dark={dark}
                fitTo={overview.fit}
                onViewport={setView}
                onOpenPhoto={(id) => void openPhoto(id)}
                onUnavailable={setMapError}
              />
            </Suspense>
            <p className="map-attrib">
              {basemap.world && (
                <>
                  © <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener noreferrer">OpenStreetMap contributors</a>
                  {" · "}
                  <a href="https://protomaps.com" target="_blank" rel="noopener noreferrer">Protomaps</a>
                  {" · "}
                </>
              )}
              Places ©{" "}
              <a href="https://www.geonames.org" target="_blank" rel="noopener noreferrer">GeoNames</a>
            </p>
          </section>
        )}
        <section className="map-list" aria-label="Photos in view" ref={listEl}>
          <p className="muted small map-count" role="status">
            {inView
              ? `${photos(inView.total)} ${mapError ? "with a location" : "in view"}${inView.truncated ? " (zoom in to see them all)" : ""}`
              : "Looking for photos…"}
          </p>
          {items.length === 0 && done && !loading && inView && inView.total === 0 && (
            <p className="muted center small">Nothing here. Pan or zoom the map.</p>
          )}
          <Grid
            items={items}
            onOpen={(i) => setLightbox({ items, index: i })}
            onThumbError={refreshThumb}
            onFavorite={(id, favorite) => {
              const it = items.find((i) => i.id === id);
              if (it) void setFavorite(it, favorite);
            }}
          />
          {listError && (
            <p className="center small" role="alert">
              {listError}{" "}
              <button type="button" className="btn btn-quiet" onClick={() => void loadMore()}>
                Retry
              </button>
            </p>
          )}
          <div ref={sentinel} className="map-sentinel" aria-hidden="true" />
        </section>
      </div>
      {lightbox && (
        <Lightbox
          items={lightbox.items}
          index={lightbox.index}
          onIndexChange={(index) => setLightbox((lb) => lb && { ...lb, index })}
          onClose={() => setLightbox(null)}
          loadDetail={api.getAsset}
          onTrash={(it) => void trash(it)}
          onAddToAlbum={(it) => setAddIds([it.id])}
          onFavorite={(it, favorite) => void setFavorite(it, favorite)}
        />
      )}
      {addIds && (
        <AddToAlbumDialog
          ids={addIds}
          onClose={() => setAddIds(null)}
          onDone={(r) => {
            const requested = addIds.length;
            setAddIds(null);
            show({ message: addedMessage(r, requested) });
          }}
        />
      )}
      {toast}
    </div>
  );
}
