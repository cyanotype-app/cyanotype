import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { api, ApiError, isScope, type AssetListItem, type ParsedChip, type Person, type SearchItem, type SearchParams, type SearchResult, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { FilterSheet } from "../components/FilterSheet";
import { Grid } from "../components/Grid";
import { LibraryTabs } from "../components/LibraryTabs";
import { Lightbox } from "../components/Lightbox";
import { useProcessing } from "../components/ProcessingStatus";
import { SelectionBar } from "../components/SelectionBar";
import { RateTools } from "../components/RateTools";
import { ZipButton } from "../components/ZipButton";
import { TagTools } from "../components/TagTools";
import { ShareDialog, shareTargetOf, type ShareTarget } from "../components/ShareDialog";
import { useToast } from "../components/Toast";
import { filterablePeople, peopleIdsOf } from "../peopleText";
import { usePeopleEnabled } from "../usePeople";
import {
  addSkip,
  chipsOf,
  hasFilters,
  indexingNote,
  indexingPercent,
  isEmptySearch,
  loadRecent,
  paramsFromUrl,
  reindexNote,
  similarUrl,
  urlFromParams,
} from "../searchText";
import { allPhotos, collapsedStats, indexResults, patchPhoto, removePhotos, selectionIds, visiblePhotos } from "../burst";
import { useSelection } from "../selection";
import { useAssetPages } from "../useAssetPages";

const PAGE = 60;
const photos = (n: number) => `${n.toLocaleString("en-US")} photo${n === 1 ? "" : "s"}`;
const EXAMPLES = ["dog on the beach", "birthday cake", "red car at night", "mountains in the snow"];

/** `/search?q=...&from=...`: natural-language search and filters over the whole library, in the library's own grid. */
export function SearchPage() {
  const [sp] = useSearchParams();
  const enabled = usePeopleEnabled();
  const params = useMemo(() => {
    const p = paramsFromUrl(sp);
    // With People off a `people` in the address is not a filter: nothing about people is shown or sent.
    if (enabled !== true) delete p.people;
    return p;
  }, [sp, enabled]);
  const key = urlFromParams(params);
  // (A link with people waits for the answer, so the first search is the right one.)
  if (enabled === null && sp.get("people")) {
    return (
      <p className="center muted" role="status">
        Searching…
      </p>
    );
  }
  // Everything below starts over when the search changes.
  return <SearchView key={key} params={params} />;
}

const withoutQuery = (p: SearchParams): Omit<SearchParams, "q" | "skip" | "similar"> => {
  const rest = { ...p };
  delete rest.q;
  delete rest.skip;
  delete rest.similar;
  return rest;
};

/** The filters a parsed query adds to the explicit ones (a date chip's range, a place chip's name). */
function withParsed(filters: Omit<SearchParams, "q" | "skip" | "similar">, chips: ParsedChip[] | undefined): Omit<SearchParams, "q" | "skip" | "similar"> {
  const out = { ...filters };
  for (const c of chips ?? []) {
    if (c.kind === "date" && c.from && c.to) {
      out.from = c.from;
      out.to = c.to;
    }
    if (c.kind === "place" && c.value) out.place = c.value;
    if (c.kind === "people" && c.people?.length) out.people = c.people.map((p) => p.id).join(",");
  }
  return out;
}

function SearchView({ params }: { params: SearchParams }) {
  const navigate = useNavigate();
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [albumTitle, setAlbumTitle] = useState<string | undefined>();
  // The owner's people, for the filter sheet and the names on the chips (asked only while People is on).
  const peopleOn = usePeopleEnabled() === true;
  const [people, setPeople] = useState<Person[] | undefined>();
  const filters = withoutQuery(params);
  const q = params.q;

  useEffect(() => {
    if (!params.album) return;
    let live = true;
    api.getAlbum(params.album).then(
      (a) => live && setAlbumTitle(a.title),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [params.album]);

  useEffect(() => {
    if (!peopleOn) return;
    let live = true;
    api.listPeople().then(
      (r) => live && setPeople(r.items),
      () => live && setPeople([]),
    );
    return () => {
      live = false;
    };
  }, [peopleOn]);

  const apply = (next: Omit<SearchParams, "q">) => {
    setFiltersOpen(false);
    const qs = urlFromParams({ ...next, q, skip: params.skip, similar: params.similar });
    navigate(qs ? `/search?${qs}` : "/search");
  };
  const clear = (keys: (keyof SearchParams)[]) => {
    const next = { ...params };
    for (const k of keys) delete next[k];
    const qs = urlFromParams(next);
    navigate(qs ? `/search?${qs}` : "/search");
  };
  const chips = chipsOf(filters, albumTitle, filterablePeople(people ?? []));

  return (
    <div className="library search-page">
      <h1 className="sr-only">Search</h1>
      <LibraryTabs />
      <div className="search-head">
        <button
          type="button"
          className="btn btn-quiet search-filters-btn"
          aria-haspopup="dialog"
          onClick={() => setFiltersOpen(true)}
        >
          <FilterIcon />
          Filters{chips.length > 0 ? ` (${chips.length})` : ""}
        </button>
        {params.similar && <SimilarChip id={params.similar} onRemove={() => clear(["similar"])} />}
        <ul className="chips" aria-label="Active filters">
          {chips.map((c) => (
            <li key={c.clears.join()}>
              <span className="chip">
                <span className="chip-label">{c.label}</span>
                <button type="button" className="chip-x" aria-label={`Remove filter ${c.label}`} onClick={() => clear(c.clears)}>
                  <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" aria-hidden="true">
                    <path d="m6 6 12 12M18 6 6 18" />
                  </svg>
                </button>
              </span>
            </li>
          ))}
        </ul>
        {hasFilters(params) && (
          <button type="button" className="btn btn-quiet small" onClick={() => clear(Object.keys(filters).filter((k) => k !== "skip") as (keyof SearchParams)[])}>
            Clear filters
          </button>
        )}
      </div>
      {isEmptySearch(params) ? <Intro /> : <Results params={params} />}
      {filtersOpen && (
        <FilterSheet
          value={filters}
          onApply={apply}
          onClose={() => setFiltersOpen(false)}
          people={peopleOn ? filterablePeople(people ?? []) : undefined}
        />
      )}
    </div>
  );
}

/** "More like this": the photo the list is similar to, with its thumbnail; removing it ends the similar mode. */
function SimilarChip({ id, onRemove }: { id: string; onRemove: () => void }) {
  const [source, setSource] = useState<{ name: string; thumb: string | null } | null>(null);
  useEffect(() => {
    let live = true;
    api.getAsset(id).then(
      (d) => live && setSource({ name: d.original_name, thumb: d.thumb_url }),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [id]);
  return (
    <span className="chip chip-photo" aria-label="Similar to">
      {source?.thumb ? <img src={source.thumb} alt="" className="chip-thumb" /> : <span className="chip-thumb" aria-hidden="true" />}
      <span className="chip-label">Similar to {source?.name ?? "photo"}</span>
      <button type="button" className="chip-x" aria-label="Remove similar photo" onClick={onRemove}>
        <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" aria-hidden="true">
          <path d="m6 6 12 12M18 6 6 18" />
        </svg>
      </button>
    </span>
  );
}

/** Before anything is typed: how to search, examples to tap, and this browser's recent searches. */
function Intro() {
  const navigate = useNavigate();
  const recent = useMemo(() => loadRecent(), []);
  const { status } = useProcessing();
  const note = reindexNote(status) ?? indexingNote(status);
  const go = (q: string) => navigate(`/search?${urlFromParams({ q })}`);
  return (
    <div className="empty search-intro">
      <h2>Search your photos</h2>
      <p>
        Describe what you are looking for, like <em>dog on the beach</em> or <em>birthday cake</em>, or use Filters for
        dates, places, cameras and more. Everything is matched on your own server: nothing leaves it.
      </p>
      <ul className="chips" aria-label="Examples">
        {EXAMPLES.map((e) => (
          <li key={e}>
            <button type="button" className="chip chip-button" onClick={() => go(e)}>
              {e}
            </button>
          </li>
        ))}
      </ul>
      {recent.length > 0 && (
        <>
          <h3 className="small muted">Recent searches</h3>
          <ul className="chips" aria-label="Recent searches">
            {recent.map((r) => (
              <li key={r}>
                <button type="button" className="chip chip-button" onClick={() => go(r)}>
                  {r}
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
      {note && <IndexingNote status={status} />}
    </div>
  );
}

function IndexingNote({ status }: { status: ReturnType<typeof useProcessing>["status"] }) {
  const reindex = reindexNote(status);
  const note = reindex ?? indexingNote(status);
  const percent = reindex ? (status?.embedding?.percent ?? null) : indexingPercent(status);
  if (!note) return null;
  return (
    <section className="indexing" aria-label={reindex ? "Search re-indexing" : "Search indexing"}>
      <p className="indexing-label small" role="status">
        {note}
      </p>
      {percent !== null && (
        <div className="processing-bar" role="progressbar" aria-label={reindex ? "Search re-indexing progress" : "Search indexing progress"} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
          <span style={{ width: `${percent}%` }} />
        </div>
      )}
    </section>
  );
}

type Meta = {
  ranked: boolean;
  total: number | null;
  parsed: SearchResult["parsed"];
  weaker: SearchResult["weaker"];
  strongCount: number | null;
  reindexing: SearchResult["reindexing"];
};

function Results({ params }: { params: SearchParams }) {
  const navigate = useNavigate();
  const { setAnonymous } = useAuth();
  const [open, setOpen] = useState<number | null>(null);
  const [meta, setMeta] = useState<Meta>({
    ranked: Boolean(params.q || params.similar),
    total: null,
    parsed: null,
    weaker: null,
    strongCount: null,
    reindexing: null,
  });
  // "Show weaker matches": the next page continues right after the last strong match, with the weaker tier included.
  const [weakerShown, setWeakerShown] = useState(false);
  const weakerRef = useRef<{ on: boolean; cursor: string | null }>({ on: false, cursor: null });
  // Bursts open inline (by their cover's id); a collapsed one is a single tile that selects all of its photos.
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [confirmed, setConfirmed] = useState(false);
  const [addIds, setAddIds] = useState<{ target: Target; count: number; save?: boolean } | null>(null);
  const [shareTarget, setShareTarget] = useState<ShareTarget | null>(null);
  const [busy, setBusy] = useState(false);
  const { toast, show } = useToast();
  const { status } = useProcessing();

  const fetchPage = useCallback(
    async (cursor: string | null) => {
      const weaker = weakerRef.current;
      const first = !cursor && !weaker.on;
      const r = await api.search(params, {
        cursor: cursor ?? (weaker.on ? weaker.cursor : null),
        limit: PAGE,
        weaker: weaker.on,
      });
      if (first) {
        setMeta({
          ranked: r.ranked,
          total: r.total ?? null,
          parsed: r.parsed ?? null,
          weaker: r.weaker ?? null,
          strongCount: r.strong_count ?? null,
          reindexing: r.reindexing ?? null,
        });
      }
      // The strong pages end where the weaker ones begin: remember where to continue from.
      if (!weaker.on && r.weaker) weakerRef.current = { on: false, cursor: r.weaker.cursor };
      return { items: r.items, next_cursor: r.next_cursor, next_before: null };
    },
    [params],
  );
  const { items, setItems, done, started, loading, error, loadMore, sentinel, reset: resetPages } = useAssetPages(fetchPage);
  // Photos change inside bursts too (a favorite, a fresh thumbnail URL).
  const refreshItem = useCallback(
    (id: string, patch: Partial<AssetListItem>) => setItems((cur) => patchPhoto(cur as SearchItem[], id, patch)),
    [setItems],
  );
  // Starting over (after a bulk change) starts from the strong matches again.
  const reset = useCallback(() => {
    weakerRef.current = { on: false, cursor: null };
    setWeakerShown(false);
    setExpanded(new Set());
    resetPages();
  }, [resetPages]);
  // Every photo of the results, those hidden in collapsed bursts included: what selecting and "select all" work on.
  const everyPhoto = useMemo(() => allPhotos(items as SearchItem[]), [items]);
  const itemIds = useMemo(() => everyPhoto.map((i) => i.id), [everyPhoto]);
  // What the grid and the lightbox show.
  const tiles = useMemo(() => visiblePhotos(items as SearchItem[], expanded), [items, expanded]);
  const byId = useMemo(() => indexResults(items as SearchItem[]), [items]);
  const hidden = useMemo(() => collapsedStats(items as SearchItem[], expanded), [items, expanded]);
  const toggleBurst = useCallback(
    (id: string) =>
      setExpanded((cur) => {
        const next = new Set(cur);
        if (!next.delete(id)) next.add(id);
        return next;
      }),
    [],
  );
  // A filter-only search can select every match (the server resolves them); a ranked one selects what is loaded.
  const sel = useSelection(itemIds, meta.ranked ? undefined : (meta.total ?? undefined));
  // What "select all" selects for a filter-only search: the explicit filters and what was read out of the query.
  const scope = useMemo(
    () => ({ search: withParsed(withoutQuery(params), meta.parsed?.chips) }) as const,
    [params, meta.parsed],
  );
  const showWeaker = () => {
    weakerRef.current = { on: true, cursor: weakerRef.current.cursor };
    setWeakerShown(true);
    void loadMore();
  };
  const removeChip = (c: ParsedChip) => {
    const qs = urlFromParams({ ...params, skip: addSkip(params.skip, c.kind) });
    navigate(`/search?${qs}`);
  };

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  const setFavorite = useCallback(
    async (ids: string[], favorite: boolean) => {
      for (const id of ids) refreshItem(id, { favorite });
      try {
        await api.favoriteAssets(ids, favorite);
      } catch (e) {
        for (const id of ids) refreshItem(id, { favorite: !favorite });
        failure(e, favorite ? "Couldn’t add to favorites." : "Couldn’t remove from favorites.");
      }
    },
    [refreshItem, failure],
  );

  const rateOne = useCallback(
    async (it: AssetListItem, rating: number) => {
      const was = it.rating ?? 0;
      refreshItem(it.id, { rating });
      try {
        await api.rateAssets([it.id], rating);
      } catch (e) {
        refreshItem(it.id, { rating: was });
        failure(e, "Couldn’t rate.");
      }
    },
    [refreshItem, failure],
  );

  const favoriteSelected = useCallback(async () => {
    const target = sel.target(scope);
    const ids = everyPhoto.filter((i) => sel.selected.has(i.id));
    const value = sel.all ? true : !ids.every((i) => i.favorite === true);
    sel.setSelecting(false);
    if (isScope(target)) {
      setBusy(true);
      try {
        const r = await api.favoriteAssets(target, value);
        reset();
        show({ message: `${r.count.toLocaleString("en-US")} ${value ? "added to" : "removed from"} favorites` });
      } catch (e) {
        failure(e, "Couldn’t update favorites.");
      } finally {
        setBusy(false);
      }
    } else {
      await setFavorite(target as string[], value);
      show({ message: `${target.length} ${value ? "added to" : "removed from"} favorites` });
    }
  }, [sel, scope, everyPhoto, setFavorite, reset, show, failure]);

  const undoTrash = useCallback(
    async (target: Target, count: number) => {
      try {
        await api.restoreAssets(target);
        reset();
        show({ message: `${count.toLocaleString("en-US")} restored` });
      } catch (e) {
        failure(e, "Couldn’t undo. They are still in the trash.");
      }
    },
    [reset, show, failure],
  );
  const trashSelected = useCallback(async () => {
    const target = sel.target(scope);
    sel.setSelecting(false);
    setBusy(true);
    try {
      const r = await api.trashAssets(target);
      if (isScope(target)) reset();
      else {
        const gone = new Set(target);
        setItems((cur) => removePhotos(cur as SearchItem[], gone));
      }
      setOpen(null);
      const back: Target = isScope(target) ? (r.batch ? { scope: { trash_batch: r.batch }, except: [] } : []) : target;
      show({
        message: `${r.count.toLocaleString("en-US")} moved to trash`,
        action: { label: "Undo", onClick: () => void undoTrash(back, r.count) },
      });
    } catch (e) {
      failure(e, "Couldn’t move to trash.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, reset, setItems, show, failure, undoTrash]);

  const trashOne = useCallback(
    async (it: AssetListItem) => {
      const gone = new Set([it.id]);
      const left = visiblePhotos(removePhotos(items as SearchItem[], gone), expanded).length;
      setItems((cur) => removePhotos(cur as SearchItem[], gone));
      setOpen((o) => (o === null || left <= 0 ? null : Math.min(o, left - 1)));
      try {
        await api.trashAssets([it.id]);
      } catch (e) {
        reset();
        failure(e, "Couldn’t move to trash.");
        return;
      }
      show({ message: "1 moved to trash", action: { label: "Undo", onClick: () => void undoTrash([it.id], 1) } });
    },
    [items, expanded, setItems, reset, show, failure, undoTrash],
  );

  const unpairItem = useCallback(
    async (it: AssetListItem) => {
      try {
        await api.unpair(it.id);
      } catch (e) {
        failure(e, "Couldn’t unpair.");
        return;
      }
      refreshItem(it.id, { paired: [] });
      show({ message: "Unpaired. They show as separate photos now." });
    },
    [failure, show, refreshItem],
  );

  useEffect(() => {
    if (open !== null || confirmed || addIds !== null || shareTarget !== null) return;
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "Escape" && sel.selecting) {
        e.preventDefault();
        sel.escape();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, confirmed, addIds, shareTarget, sel]);

  const openAt = useCallback(
    (i: number) => {
      setOpen(i);
      if (i >= tiles.length - 3 && !done && !error) void loadMore();
    },
    [tiles.length, done, error, loadMore],
  );

  const indexing = params.q ? indexingNote(status) : null;
  // An older model answers while the configured one indexes the library: say so (the response knows first).
  const reindexing = params.q && (meta.reindexing || status?.embedding?.reindexing);
  const total = meta.total ?? everyPhoto.length;
  // (the pages are search results: they carry a tier)
  const strongLoaded = items.filter((i) => (i as SearchItem).tier !== "weak").length;
  const weakLoaded = items.length - strongLoaded;
  const matches = params.similar
    ? `${strongLoaded.toLocaleString("en-US")} similar photo${strongLoaded === 1 ? "" : "s"}`
    : `${strongLoaded.toLocaleString("en-US")} match${strongLoaded === 1 ? "" : "es"}`;
  const shown = meta.ranked
    ? `${matches}${weakLoaded > 0 ? `, ${weakLoaded.toLocaleString("en-US")} weaker` : ""}${done ? "" : " so far"}`
    : photos(meta.total ?? items.length);
  const noResults = started && items.length === 0 && !error;
  const noModel = error !== null && /not available|not installed/i.test(error);
  // "More like this" of a photo the worker has not embedded yet (the server answers 409).
  const notIndexed = Boolean(params.similar) && error !== null && /still being indexed/i.test(error);
  const weakerCount = meta.weaker?.count ?? 0;
  const weakerButton = (
    <button type="button" className="btn btn-quiet weaker-btn" onClick={showWeaker}>
      Show weaker matches ({weakerCount.toLocaleString("en-US")})
    </button>
  );
  const chips = meta.parsed?.chips ?? [];

  if (!started) return <p className="center muted" role="status">Searching…</p>;

  return (
    <>
      {(indexing || reindexing) && <IndexingNote status={status} />}
      {chips.length > 0 && (
        <ul className="chips parsed-chips" aria-label="Understood from your search">
          {chips.map((c) => (
            <li key={c.kind}>
              <span className="chip" title={`Read “${c.matched}” as ${c.kind === "people" ? "people" : `a ${c.kind}`}`}>
                <span className="chip-label">{c.label}</span>
                <button type="button" className="chip-x" aria-label={`Remove ${c.kind} ${c.label}`} onClick={() => removeChip(c)}>
                  <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" aria-hidden="true">
                    <path d="m6 6 12 12M18 6 6 18" />
                  </svg>
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
      {noResults && (
        <div className="empty" role="status">
          <h2>
            {weakerCount > 0 && !weakerShown ? "No close matches" : params.similar ? "No similar photos" : "Nothing matched"}
          </h2>
          <p>
            {weakerCount > 0 && !weakerShown
              ? "Nothing in your library matches this closely. There are weaker matches."
              : params.similar
                ? "Nothing else in your library looks like this photo, or the filters leave nothing. Remove a filter to widen it."
                : params.q
                ? "Try describing the scene with other words, or remove a filter."
                : "No photo has all of these. Remove a filter to widen the search."}
          </p>
          {weakerCount > 0 && !weakerShown && weakerButton}
          {indexing && <p className="muted small">Photos that are still being indexed cannot match a description yet.</p>}
        </div>
      )}
      {notIndexed && (
        <div className="empty" role="status">
          <h2>This photo is still being indexed</h2>
          <p className="muted">Similar photos can be found once it has been indexed, which usually takes a few minutes after an upload.</p>
          <button type="button" className="btn btn-primary" onClick={() => reset()}>
            Try again
          </button>
        </div>
      )}
      {error && !notIndexed && (
        <div className="empty" role="alert">
          <h2>{noModel ? "Search isn’t available" : "Couldn’t search"}</h2>
          <p className="muted">
            {noModel
              ? "This server has no search model installed. Filters still work; ask whoever runs the server to install the model."
              : error}
          </p>
          {!noModel && (
            <button type="button" className="btn btn-primary" onClick={() => (items.length ? void loadMore() : reset())}>
              Try again
            </button>
          )}
        </div>
      )}
      {items.length > 0 && (
        <>
          <div className="library-bar">
            <p className="muted small search-count" role="status">
              {shown}
            </p>
            <button
              type="button"
              className="btn btn-quiet"
              aria-pressed={sel.selecting}
              onClick={() => sel.setSelecting(!sel.selecting)}
            >
              Select
            </button>
            {/* The photos of the people filter (a filter-only search: the server can name them all) as an album of their own. */}
            {!meta.ranked && (peopleIdsOf(params.people).length > 0 || chips.some((c) => c.kind === "people")) && (meta.total ?? 0) > 0 && (
              <button
                type="button"
                className="btn btn-quiet"
                disabled={busy}
                onClick={() => setAddIds({ target: { scope, except: [] }, count: meta.total ?? 0, save: true })}
              >
                Save as album
              </button>
            )}
          </div>
          {sel.selecting && (
            <SelectionBar
              count={sel.count}
              total={total}
              onSelectAll={sel.selectAll}
              onClear={sel.clear}
              onDone={() => sel.setSelecting(false)}
              note={
                hidden.bursts > 0
                  ? `A burst (+N) selects all its photos: ${hidden.photos.toLocaleString("en-US")} more in ${hidden.bursts.toLocaleString("en-US")} ${hidden.bursts === 1 ? "burst" : "bursts"}.`
                  : null
              }
            >
              <button type="button" className="btn btn-quiet" disabled={sel.count === 0 || busy} onClick={() => void favoriteSelected()}>
                Favorite
              </button>
              <ZipButton
                target={sel.target(scope)}
                disabled={sel.count === 0}
                onStarted={(message) => show({ message })}
                onError={(e) => failure(e, "Couldn’t download.")}
              />
              <RateTools
                target={sel.target(scope)}
                count={sel.count}
                disabled={sel.count === 0 || busy}
                onDone={(rating, _n, message) => {
                  const picked = sel.all ? null : new Set(sel.ids);
                  sel.setSelecting(false);
                  // Loaded photos change at once; a whole-view rating starts the list over.
                  if (picked) for (const id of picked) refreshItem(id, { rating });
                  else reset();
                  show({ message });
                }}
                onError={(e) => failure(e, "Couldn’t rate.")}
              />
              <TagTools
                target={sel.target(scope)}
                count={sel.count}
                disabled={sel.count === 0 || busy}
                onDone={(message) => {
                  sel.setSelecting(false);
                  // A tag filter in force changes which photos match: start over from the first page.
                  reset();
                  show({ message });
                }}
              />
              <button
                type="button"
                className="btn btn-quiet"
                disabled={sel.count === 0 || busy}
                onClick={() => setAddIds({ target: sel.target(scope), count: sel.count })}
              >
                Add to album
              </button>
              <button
                type="button"
                className="btn btn-quiet"
                disabled={sel.count === 0 || busy}
                onClick={() => setShareTarget(shareTargetOf(sel.target(scope), sel.count))}
              >
                Share
              </button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={sel.count === 0 || busy}
                onClick={() => (sel.all ? setConfirmed(true) : void trashSelected())}
              >
                Move to trash
              </button>
            </SelectionBar>
          )}
          <Grid
            items={tiles}
            onOpen={openAt}
            onThumbError={(id) =>
              void api.getAsset(id).then(
                (d) => refreshItem(id, { thumb_url: d.thumb_url, thumbhash: d.thumbhash }),
                () => undefined,
              )
            }
            selecting={sel.selecting}
            selected={sel.selected}
            // A collapsed burst is one tile that stands for all of its photos; an open one is its photos one by one.
            onSelect={(id, shift) => {
              const found = byId.get(id);
              const ids = found && found.cover.id === id ? selectionIds(found.cover, expanded) : [id];
              if (ids.length > 1) sel.selectMany(ids);
              else sel.select(id, shift);
            }}
            onLongPress={(id) => {
              const found = byId.get(id);
              const ids = found && found.cover.id === id ? selectionIds(found.cover, expanded) : [id];
              if (ids.length > 1) sel.beginMany(ids);
              else sel.begin(id);
            }}
            burst={(it) => {
              const found = byId.get(it.id);
              const n = found && found.cover.id === it.id ? (found.cover.burst?.members.length ?? 0) : 0;
              return n > 0 ? { more: n, expanded: expanded.has(it.id), onToggle: () => toggleBurst(it.id) } : null;
            }}
            inBurst={(it) => {
              const found = byId.get(it.id);
              return found !== undefined && found.cover.id !== it.id;
            }}
            onFavorite={(id, favorite) => void setFavorite([id], favorite)}
          />
        </>
      )}
      {done && items.length > 0 && weakerCount > 0 && !weakerShown && !error && (
        <div className="weaker-more">{weakerButton}</div>
      )}
      <div ref={sentinel} className="sentinel" aria-hidden="true" />
      {loading && items.length > 0 && <p className="center muted small" role="status">Loading more…</p>}
      {open !== null && tiles.length > 0 && (
        <Lightbox
          items={tiles}
          index={Math.min(open, tiles.length - 1)}
          onIndexChange={openAt}
          onClose={() => setOpen(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onTrash={(it) => void trashOne(it)}
          onAddToAlbum={(it) => setAddIds({ target: [it.id], count: 1 })}
          onShare={(it) => setShareTarget({ kind: "photos", ids: [it.id] })}
          onUnpair={(it) => void unpairItem(it)}
          onFavorite={(it, favorite) => void setFavorite([it.id], favorite)}
          onRate={(it, n) => void rateOne(it, n)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
      {confirmed && (
        <ConfirmTrash
          count={sel.count}
          onCancel={() => setConfirmed(false)}
          onConfirm={() => {
            setConfirmed(false);
            void trashSelected();
          }}
        />
      )}
      {addIds !== null && (
        <AddToAlbumDialog
          ids={addIds.target}
          count={addIds.count}
          mode={addIds.save ? "save" : "add"}
          onClose={() => setAddIds(null)}
          onDone={(r) => {
            const requested = addIds.count;
            setAddIds(null);
            sel.setSelecting(false);
            show({ message: addedMessage(r, requested) });
          }}
        />
      )}
      {shareTarget !== null && <ShareDialog target={shareTarget} onClose={() => setShareTarget(null)} />}
      {toast}
    </>
  );
}


function ConfirmTrash({ count, onCancel, onConfirm }: { count: number; onCancel: () => void; onConfirm: () => void }) {
  return (
    <ConfirmDialog title={`Move ${photos(count)} to trash?`} confirmLabel="Move to trash" onCancel={onCancel} onConfirm={onConfirm}>
      <p>You can restore {count === 1 ? "it" : "them"} from the trash.</p>
    </ConfirmDialog>
  );
}

function FilterIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
      <path d="M4 6h16M7 12h10M10 18h4" />
    </svg>
  );
}
