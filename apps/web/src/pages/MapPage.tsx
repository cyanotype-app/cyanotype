import { MapPin } from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { pointFromSearch } from "../detailsText";
import { api, ApiError, type AssetListItem, type MapCluster, type TilesInstalled } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ShareDialog } from "../components/ShareDialog";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { useToast } from "../components/Toast";
import { similarUrl } from "../searchText";
import { bboxParam, clusterZoom, extent, WORLD, type Bounds } from "../map/geo";
import type { MapViewport } from "../map/MapCanvas";
import { MapPanel } from "../map/MapPanel";
import { useViewportPhotos } from "../map/useViewportPhotos";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { useDesktop } from "../ui/useMedia";

// maplibre-gl and pmtiles are big; they load only when the map page opens.
const MapCanvas = lazy(() => import("../map/MapCanvas"));

/** The view is acted on this long after the user stops moving it. */
const SETTLE_MS = 200;
const darkQuery = () => window.matchMedia?.("(prefers-color-scheme: dark)");

const photos = (n: number) => `${n.toLocaleString()} photo${n === 1 ? "" : "s"}`;

/** Where the geotagged photos are: a clustered map and, beside or under it, the photos in the part on screen. */
export function MapPage() {
  const navigate = useNavigate();
  const desktop = useDesktop();
  // /map?lat=&lon= (from a photo's details) opens centred on that spot instead of on the whole library.
  const [search] = useSearchParams();
  const focus = pointFromSearch(search);
  const focusLat = focus?.lat;
  const focusLon = focus?.lon;
  const focusBounds = useMemo<Bounds | null>(
    () => (focusLat === undefined || focusLon === undefined ? null : { west: focusLon, south: focusLat, east: focusLon, north: focusLat }),
    [focusLat, focusLon],
  );
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
  const [shareIds, setShareIds] = useState<string[] | null>(null);
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
      <>
        <PageHeader title="Places" />
        <Message title="Couldn’t load the map" body={loadError} alert />
      </>
    );
  }
  if (!overview || !tiles) {
    return (
      <>
        <PageHeader title="Places" />
        <div className="mx-4 mt-4 h-[50dvh] animate-pulse rounded-card bg-surface md:mx-8" role="status" aria-label="Loading the map" />
      </>
    );
  }
  const basemap = tiles;
  if (overview.total === 0) {
    return (
      <>
        <PageHeader title="Places" />
        <Message
          icon={<MapPin className="size-7" aria-hidden="true" />}
          title="No photos with a location yet"
          body="Photos that carry a GPS position in their EXIF data show up here once they are processed. Phone photos usually do; most camera files do not."
        />
      </>
    );
  }

  // What the bars and the panel cover, so the first view is fitted to what is left.
  const inset = desktop ? { top: 88, right: 368, bottom: 0, left: 0 } : { top: 72, right: 0, bottom: 100, left: 0 };
  const notice = mapError
    ? { text: `The map can’t be drawn in this browser (${mapError}). The photos with a location are listed below.`, alert: true }
    : !basemap.world
      ? {
          text: "No basemap is installed on this server, so photos are drawn on a plain background. The admin can add one (see “Map” in the deploy guide).",
          alert: false,
        }
      : null;

  return (
    <div
      className={
        "relative h-[calc(100dvh-var(--tabbar-h))] overflow-hidden bg-surface " +
        "[--hdr:calc(4.25rem_+_env(safe-area-inset-top,0px))] md:[--hdr:5.25rem]"
      }
    >
      <PageHeader title="Places" className="absolute inset-x-0 top-0 border-line" />
      {!mapError && (
        <section
          aria-label="Map of your photos"
          className={
            "absolute inset-0 [&_.maplibregl-ctrl-top-right]:top-(--hdr)! " +
            "md:[&_.maplibregl-ctrl-top-right]:right-[22.75rem]! [&_.maplibregl-ctrl-top-right]:mr-2.5"
          }
        >
          <Suspense
            fallback={
              <p className="flex h-full items-center justify-center text-muted" role="status">
                Loading the map…
              </p>
            }
          >
            <MapCanvas
              clusters={clusters}
              basemap={basemap}
              dark={dark}
              fitTo={focusBounds ?? overview.fit}
              onViewport={setView}
              onOpenPhoto={(id) => void openPhoto(id)}
              onUnavailable={setMapError}
              inset={inset}
            />
          </Suspense>
          <p className="absolute top-(--hdr) left-0 z-[2] mt-1 max-w-[70%] rounded-r-md bg-bg/80 px-2 py-0.5 text-[0.6875rem] leading-snug text-muted backdrop-blur md:top-auto md:bottom-0 md:mt-0 md:max-w-[60%] md:rounded-r-none md:rounded-tr-md">
            {basemap.world && (
              <>
                © <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener noreferrer" className="text-inherit">OpenStreetMap contributors</a>
                {" · "}
                <a href="https://protomaps.com" target="_blank" rel="noopener noreferrer" className="text-inherit">Protomaps</a>
                {" · "}
              </>
            )}
            Places ©{" "}
            <a href="https://www.geonames.org" target="_blank" rel="noopener noreferrer" className="text-inherit">GeoNames</a>
          </p>
        </section>
      )}
      {notice && (
        <p
          data-map-notice=""
          role={notice.alert ? "alert" : "note"}
          className="absolute inset-x-3 top-(--hdr) z-[3] mt-2 rounded-card bg-card px-4 py-3 text-sm text-muted shadow-float [overflow-wrap:anywhere] md:right-auto md:max-w-md"
        >
          {notice.text}
        </p>
      )}
      <MapPanel
        label="Photos in view"
        listRef={listEl}
        open={Boolean(mapError)}
        heading={
          <p className="text-[0.9375rem] font-medium" role="status">
            {inView
              ? `${photos(inView.total)} ${mapError ? "with a location" : "in view"}${inView.truncated ? " (zoom in to see them all)" : ""}`
              : "Looking for photos…"}
          </p>
        }
      >
        {items.length === 0 && done && !loading && inView && inView.total === 0 && (
          <p className="px-4 py-8 text-center text-sm text-muted">Nothing here. Pan or zoom the map.</p>
        )}
        <div className="[&_.grid]:grid-cols-[repeat(auto-fill,minmax(6.5rem,1fr))] [&_.grid]:gap-0.5">
          <Grid
            items={items}
            onOpen={(i) => setLightbox({ items, index: i })}
            onThumbError={refreshThumb}
            onFavorite={(id, favorite) => {
              const it = items.find((i) => i.id === id);
              if (it) void setFavorite(it, favorite);
            }}
          />
        </div>
        {listError && (
          <p className="px-4 py-4 text-center text-sm" role="alert">
            {listError}{" "}
            <Button size="sm" onClick={() => void loadMore()}>
              Retry
            </Button>
          </p>
        )}
        <div ref={sentinel} className="h-px" aria-hidden="true" />
      </MapPanel>
      {lightbox && (
        <Lightbox
          items={lightbox.items}
          index={lightbox.index}
          onIndexChange={(index) => setLightbox((lb) => lb && { ...lb, index })}
          onClose={() => setLightbox(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onTrash={(it) => void trash(it)}
          onAddToAlbum={(it) => setAddIds([it.id])}
          onShare={(it) => setShareIds([it.id])}
          onFavorite={(it, favorite) => void setFavorite(it, favorite)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
      {shareIds && <ShareDialog target={{ kind: "photos", ids: shareIds }} onClose={() => setShareIds(null)} />}
      {addIds && (
        <AddToAlbumDialog
          ids={addIds}
          count={addIds.length}
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


function Message({ title, body, icon, alert }: { title: string; body: string; icon?: ReactNode; alert?: boolean }) {
  return (
    <div className="mx-auto flex max-w-sm flex-col items-center gap-3 px-6 py-20 text-center" role={alert ? "alert" : undefined}>
      {icon && <span className="flex size-16 items-center justify-center rounded-full bg-accent/12 text-accent">{icon}</span>}
      <h2 className="text-lg font-semibold tracking-tight">{title}</h2>
      <p className="text-[0.9375rem] text-muted">{body}</p>
    </div>
  );
}
