import { MoreHorizontal } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, type Trip } from "../api";
import { useAuth } from "../auth";
import { dateRange } from "../collections/text";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { photoCount } from "../albumText";
import { similarUrl } from "../searchText";
import { PageHeader } from "../shell/PageHeader";
import { SlideshowItem } from "../collections/SlideshowItem";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { itemsSource } from "../slideshow/sources";
import { Button } from "../ui/Button";
import { Menu } from "../ui/Menu";
import { useAssetPages } from "../useAssetPages";

const PAGE = 60;
const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** `<from>_<to>` (a trip's id) as two dates, or null when it is not one. */
export function parseTripId(id: string): { from: string; to: string } | null {
  const [from, to, ...rest] = id.split("_");
  if (rest.length > 0 || !from || !to || !DAY.test(from) || !DAY.test(to) || to < from) return null;
  return { from, to };
}

/** The next calendar day, `YYYY-MM-DD`. */
function dayAfter(day: string): string {
  const [y = 0, m = 1, d = 1] = day.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d + 1)).toISOString().slice(0, 10);
}

export function TripPage() {
  const { id = "" } = useParams();
  return <TripView key={id} id={id} />;
}

/** One trip of "Recent places": its photos, newest first, under its place and dates. */
function TripView({ id }: { id: string }) {
  const range = parseTripId(id);
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  // The card that was tapped brings the trip along, so the title is there at once; a reload fetches it.
  const passed = (useLocation().state as { trip?: Trip } | null)?.trip;
  const [trip, setTrip] = useState<Trip | null>(passed?.id === id ? passed : null);
  const [open, setOpen] = useState<number | null>(null);
  const slideshow = useSlideshow();

  useEffect(() => {
    if (!range || trip) return;
    let live = true;
    // The trips that ended before the day after this one: the newest is this trip, if it is still one.
    api
      .trips({ limit: 1, before: dayAfter(range.to) })
      .then((r) => live && setTrip(r.trips.find((t) => t.id === id) ?? null))
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.status === 401) setAnonymous();
      });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const from = range?.from;
  const to = range?.to;
  const fetchPage = useCallback(
    (cursor: string | null) => api.listAssets({ from, to, memories: true, cursor, limit: PAGE }),
    [from, to],
  );
  const { items, done, started, loading, error, loadMore, sentinel, refreshItem } = useAssetPages(fetchPage);
  const refreshThumb = useCallback(
    (assetId: string) =>
      void api.getAsset(assetId).then(
        (d) => refreshItem(assetId, { thumb_url: d.thumb_url, thumbhash: d.thumbhash }),
        () => undefined,
      ),
    [refreshItem],
  );

  const back = { to: "/", label: "Collections" };
  if (!range)
    return (
      <>
        <PageHeader title="Trip" back={back} />
        <Problem title="That isn’t a trip" body="The link is not one of your trips." alert />
      </>
    );

  const count = trip?.photos ?? (done ? items.length : null);
  const subtitle = [dateRange(range.from, range.to), count === null ? null : photoCount(count)].filter(Boolean).join(" · ");
  return (
    <>
      <PageHeader
        title={trip?.title ?? "Trip"}
        subtitle={subtitle}
        back={back}
        actions={
          items.length > 0 && slideshow && (
            <Menu
              label="Trip options"
              trigger={
                <Button variant="ghost" size="icon" aria-label="More">
                  <MoreHorizontal />
                </Button>
              }
            >
              <SlideshowItem source={() => itemsSource(items, trip?.title ?? "Trip")} />
            </Menu>
          )
        }
      />
      <div className="px-1 pt-2 pb-8 md:px-6">
        {error && items.length === 0 ? (
          <Problem title="Couldn’t load the photos" body={error} alert>
            <Button variant="primary" onClick={() => void loadMore()}>
              Try again
            </Button>
          </Problem>
        ) : started && items.length === 0 ? (
          <Problem title="No photos here" body="The photos of this trip were moved out of memories or deleted.">
            <Button asChild variant="primary">
              <Link to="/">Back to Collections</Link>
            </Button>
          </Problem>
        ) : (
          <Grid items={items} onOpen={setOpen} onThumbError={refreshThumb} />
        )}
        {!started && (
          <p className="py-10 text-center text-muted" role="status">
            Loading photos…
          </p>
        )}
        {error && items.length > 0 && (
          <p className="py-4 text-center text-sm" role="alert">
            {error}{" "}
            <Button size="sm" onClick={() => void loadMore()}>
              Retry
            </Button>
          </p>
        )}
        {loading && started && <p className="sr-only" role="status">Loading more photos</p>}
        <div ref={sentinel} aria-hidden="true" className="h-px" />
      </div>
      {open !== null && items[open] && (
        <Lightbox
          items={items}
          index={open}
          onIndexChange={setOpen}
          onClose={() => setOpen(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
    </>
  );
}

function Problem({ title, body, children, alert }: { title: string; body: string; children?: ReactNode; alert?: boolean }) {
  return (
    <div className="mx-auto flex max-w-sm flex-col items-center gap-3 px-6 py-16 text-center" role={alert ? "alert" : undefined}>
      <h2 className="text-lg font-semibold tracking-tight">{title}</h2>
      <p className="text-[0.9375rem] text-muted">{body}</p>
      {children}
    </div>
  );
}
