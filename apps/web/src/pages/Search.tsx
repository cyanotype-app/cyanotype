import { Clock, Loader2, MapPin, SearchX, SlidersHorizontal, Sparkles, X, type LucideIcon } from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { api, ApiError, isScope, localToday, type AssetListItem, type Facets, type NamedPet, type ParsedChip, type Person, type SearchItem, type SearchParams, type SearchResult, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { FaceCrop } from "../components/FaceCrop";
import { FilterSheet } from "../components/FilterSheet";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { useProcessing } from "../components/ProcessingStatus";
import { SearchBar } from "../components/SearchBar";
import { SelectionBar } from "../components/SelectionBar";
import { RateTools } from "../components/RateTools";
import { ZipButton } from "../components/ZipButton";
import { TagTools } from "../components/TagTools";
import { ShareDialog, shareTargetOf, type ShareTarget } from "../components/ShareDialog";
import { useToast } from "../components/Toast";
import { filterablePeople, peopleIdsOf, personLabel } from "../peopleText";
import { filterablePets } from "../petsText";
import { NamePetSheet, petNamedToast } from "../components/NamePetSheet";
import { PageHeader } from "../shell/PageHeader";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { usePeopleEnabled } from "../usePeople";
import { usePetsEnabled } from "../usePets";
import {
  addSkip,
  chipsOf,
  clearRecent,
  dateShortcuts,
  hasFilters,
  indexingNote,
  indexingPercent,
  isEmptySearch,
  loadRecent,
  paramsFromUrl,
  quickFilters,
  reindexNote,
  similarUrl,
  urlFromParams,
  yearRange,
} from "../searchText";
import { allPhotos, collapsedStats, indexResults, patchPhoto, removePhotos, selectionIds, visiblePhotos } from "../burst";
import { useSelection } from "../selection";
import { useAssetPages } from "../useAssetPages";
import { SlideshowButton } from "../slideshow/SlideshowHost";
import { searchSource } from "../slideshow/sources";

const PAGE = 60;
const photos = (n: number) => `${n.toLocaleString("en-US")} photo${n === 1 ? "" : "s"}`;
const EXAMPLES = ["dog on the beach", "birthday cake", "red car at night", "mountains in the snow"];

/** `/search?q=...&from=...`: natural-language search and filters over the whole library, in the library's own grid. */
export function SearchPage() {
  const [sp] = useSearchParams();
  const enabled = usePeopleEnabled();
  const petsEnabled = usePetsEnabled();
  const params = useMemo(() => {
    const p = paramsFromUrl(sp);
    // With People off a `people` in the address is not a filter: nothing about people is shown or sent. The same for `pets`.
    if (enabled !== true) delete p.people;
    if (petsEnabled !== true) {
      delete p.pets;
      delete p.pet;
    }
    return p;
  }, [sp, enabled, petsEnabled]);
  const key = urlFromParams(params);
  // (A link with people or pets waits for the answer, so the first search is the right one.)
  if ((enabled === null && sp.get("people")) || (petsEnabled === null && (sp.get("pets") || sp.get("pet")))) {
    return (
      <>
        <PageHeader title="Search" />
        <Searching />
      </>
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
    if (c.kind === "pets" && c.pets) out.pets = c.pets;
    if (c.kind === "pet" && c.pet?.length) out.pet = c.pet.map((p) => p.id).join(",");
  }
  return out;
}

const chipCls = "inline-flex h-9 max-w-full items-center rounded-full text-[0.8125rem] font-medium";

function SearchView({ params }: { params: SearchParams }) {
  const navigate = useNavigate();
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [albumTitle, setAlbumTitle] = useState<string | undefined>();
  // The owner's people, for the filter sheet, the names on the chips and the suggestions (asked only while People is on).
  const peopleOn = usePeopleEnabled() === true;
  const petsOn = usePetsEnabled() === true;
  const [people, setPeople] = useState<Person[] | undefined>();
  // The pets the owner named, for the filter sheet and the names on the chips (asked only while Pets is on).
  const [namedPets, setNamedPets] = useState<NamedPet[] | undefined>();
  const filters = withoutQuery(params);
  const empty = isEmptySearch(params);

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
  useEffect(() => {
    if (!petsOn) return;
    let live = true;
    api.namedPets().then(
      (r) => live && setNamedPets(r.items),
      () => live && setNamedPets([]),
    );
    return () => {
      live = false;
    };
  }, [petsOn]);

  const go = (next: SearchParams) => {
    const qs = urlFromParams(next);
    navigate(qs ? `/search?${qs}` : "/search");
  };
  const apply = (next: Omit<SearchParams, "q">) => {
    setFiltersOpen(false);
    go({ ...next, q: params.q, skip: params.skip, similar: params.similar });
  };
  const clear = (keys: (keyof SearchParams)[]) => {
    const next = { ...params };
    for (const k of keys) delete next[k];
    go(next);
  };
  const chips = chipsOf(filters, albumTitle, filterablePeople(people ?? []), namedPets ?? []);
  const clearFilters = () => clear(Object.keys(filters).filter((k) => k !== "skip") as (keyof SearchParams)[]);
  const closeFilters = useCallback(() => setFiltersOpen(false), []);

  return (
    <>
      <PageHeader title="Search">
        <SearchBar recentsInPage={empty}>
          <Button
            variant="secondary"
            aria-haspopup="dialog"
            aria-label={`Filters${chips.length > 0 ? ` (${chips.length})` : ""}`}
            className="relative size-12 shrink-0 px-0 md:w-auto md:px-5"
            onClick={() => setFiltersOpen(true)}
          >
            <SlidersHorizontal />
            <span className="hidden md:inline" aria-hidden="true">
              Filters
            </span>
            {chips.length > 0 && (
              <span
                aria-hidden="true"
                className="absolute -top-0.5 -right-0.5 inline-flex size-5 items-center justify-center rounded-full bg-accent text-xs font-semibold text-on-accent md:static md:size-auto md:min-w-5 md:px-1"
              >
                {chips.length}
              </span>
            )}
          </Button>
        </SearchBar>
        {(chips.length > 0 || params.similar) && (
          <div className="mt-2.5 flex flex-wrap items-center gap-2">
            {params.similar && <SimilarChip id={params.similar} onRemove={() => clear(["similar"])} />}
            <ul className="contents" aria-label="Active filters">
              {chips.map((c) => (
                <li key={c.clears.join()} className="max-w-full">
                  <span className={cn(chipCls, "gap-0.5 bg-accent/12 pr-1 pl-3.5 text-accent")}>
                    <span className="truncate">{c.label}</span>
                    <RemoveButton label={`Remove filter ${c.label}`} onClick={() => clear(c.clears)} />
                  </span>
                </li>
              ))}
            </ul>
            {hasFilters(params) && (
              <Button variant="quiet" size="sm" onClick={clearFilters}>
                Clear filters
              </Button>
            )}
          </div>
        )}
      </PageHeader>
      <div className="px-4 pt-4 pb-10 md:px-8">
        {empty ? <Intro people={people} /> : <Results params={params} onClearFilters={hasFilters(params) ? clearFilters : undefined} />}
      </div>
      <FilterSheet
        open={filtersOpen}
        value={filters}
        onApply={apply}
        onClose={closeFilters}
        people={peopleOn ? filterablePeople(people ?? []) : undefined}
        pets={petsOn}
        namedPets={petsOn ? filterablePets(namedPets ?? []) : undefined}
      />
    </>
  );
}

function RemoveButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      className="inline-flex size-7 shrink-0 items-center justify-center rounded-full hover:bg-accent/15"
    >
      <X className="size-3.5" aria-hidden="true" />
    </button>
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
    <span className={cn(chipCls, "gap-2 bg-accent/12 pr-1 pl-1.5 text-accent")} aria-label="Similar to">
      {source?.thumb ? (
        <img src={source.thumb} alt="" className="size-6 shrink-0 rounded-full object-cover" />
      ) : (
        <span className="size-6 shrink-0 rounded-full bg-line" aria-hidden="true" />
      )}
      <span className="truncate">Similar to {source?.name ?? "photo"}</span>
      <RemoveButton label="Remove similar photo" onClick={onRemove} />
    </span>
  );
}

const pillCls =
  "inline-flex h-10 max-w-full items-center gap-2 rounded-full bg-surface px-4 text-[0.9375rem] text-fg no-underline " +
  "transition-[background-color,transform] duration-150 ease-soft hover:bg-line/70 active:scale-[0.97] [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-muted";

function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section>
      <div className="mb-2.5 flex items-center justify-between gap-3">
        <h2 className="text-[1.0625rem] font-semibold tracking-tight">{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}

function Pills({ label, children }: { label: string; children: ReactNode }) {
  return (
    <ul className="flex flex-wrap gap-2" aria-label={label}>
      {children}
    </ul>
  );
}

/** Before anything is typed: recent searches, and one tap into the library's people, places, kinds of photo and dates. */
function Intro({ people }: { people: Person[] | undefined }) {
  const navigate = useNavigate();
  const reduce = useReducedMotion();
  const [recent, setRecent] = useState(() => loadRecent().slice(0, 5));
  const [facets, setFacets] = useState<Facets | null>(null);
  const { status } = useProcessing();
  const note = reindexNote(status) ?? indexingNote(status);
  const go = (q: string) => navigate(`/search?${urlFromParams({ q })}`);
  const to = (p: SearchParams) => `/search?${urlFromParams(p)}`;

  useEffect(() => {
    let live = true;
    api.searchFacets().then(
      (f) => live && setFacets(f),
      () => undefined, // the shortcuts that need the library's numbers are simply not offered
    );
    return () => {
      live = false;
    };
  }, []);

  const today = localToday();
  const named = useMemo(() => [...filterablePeople(people ?? [])].sort((a, b) => b.photos - a.photos).slice(0, 8), [people]);
  const places = useMemo(() => [...(facets?.places ?? [])].sort((a, b) => b.count - a.count).slice(0, 8), [facets]);
  const kinds = useMemo(() => (facets ? quickFilters(facets) : []), [facets]);
  const years = useMemo(() => (facets?.years ?? []).slice(0, 6), [facets]);

  return (
    <motion.div
      className="mx-auto flex max-w-5xl flex-col gap-8"
      initial={reduce ? false : { opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.2, 0.8, 0.2, 1] }}
    >
      <h2 className="sr-only">Search your photos</h2>
      {note && <IndexingNote status={status} />}
      {recent.length > 0 && (
        <Section
          title="Recent searches"
          action={
            <Button
              variant="quiet"
              size="sm"
              aria-label="Clear recent searches"
              onClick={() => {
                clearRecent();
                setRecent([]);
              }}
            >
              Clear
            </Button>
          }
        >
          <ul className="divide-y divide-line overflow-hidden rounded-card bg-card ring-1 ring-line" aria-label="Recent searches">
            {recent.map((r) => (
              <li key={r}>
                <button
                  type="button"
                  className="flex h-12 w-full items-center gap-3 px-4 text-left text-[0.9375rem] transition-colors hover:bg-surface active:bg-surface"
                  onClick={() => go(r)}
                >
                  <Clock className="size-4 shrink-0 text-muted" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate">{r}</span>
                </button>
              </li>
            ))}
          </ul>
        </Section>
      )}
      {named.length > 0 && (
        <Section
          title="People"
          action={
            <Button variant="quiet" size="sm" asChild>
              <Link to="/people">All people</Link>
            </Button>
          }
        >
          <ul className="grid grid-cols-4 gap-x-3 gap-y-4 sm:grid-cols-6 md:grid-cols-8" aria-label="People">
            {named.map((p) => (
              <li key={p.id} className="min-w-0">
                <Link to={to({ people: p.id })} className="group flex flex-col items-center gap-1.5 text-fg no-underline">
                  <span className="block size-16 overflow-hidden rounded-full bg-surface transition-transform duration-150 ease-soft group-active:scale-95 md:size-[4.5rem]">
                    {p.cover ? (
                      <FaceCrop url={p.cover.preview_url} box={p.cover.box} />
                    ) : (
                      <span className="flex size-full items-center justify-center text-lg font-semibold text-muted" aria-hidden="true">
                        {personLabel(p).slice(0, 1).toUpperCase()}
                      </span>
                    )}
                  </span>
                  <span className="w-full truncate text-center text-[0.8125rem]">{personLabel(p)}</span>
                </Link>
              </li>
            ))}
          </ul>
        </Section>
      )}
      {places.length > 0 && (
        <Section title="Places">
          <Pills label="Places">
            {places.map((p) => (
              <li key={`${p.kind}:${p.value}`} className="max-w-full">
                <Link to={to({ place: p.value })} className={pillCls}>
                  <MapPin />
                  <span className="truncate">{p.value}</span>
                </Link>
              </li>
            ))}
          </Pills>
        </Section>
      )}
      {kinds.length > 0 && (
        <Section title="Kinds of photo">
          <Pills label="Kinds of photo">
            {kinds.map((k) => (
              <li key={k.key} className="max-w-full">
                <Link to={to(k.patch)} className={pillCls}>
                  <span className="truncate">{k.label}</span>
                  {k.count !== undefined && <span className="text-sm text-muted">{k.count.toLocaleString("en-US")}</span>}
                </Link>
              </li>
            ))}
          </Pills>
        </Section>
      )}
      <Section title="Dates">
        <Pills label="Dates">
          {dateShortcuts(today).map((d) => (
            <li key={d.label}>
              <Link to={to({ from: d.from, to: d.to })} className={pillCls}>
                {d.label}
              </Link>
            </li>
          ))}
          {years
            .filter((y) => y.value !== String(Number(today.slice(0, 4))) && y.value !== String(Number(today.slice(0, 4)) - 1))
            .map((y) => (
              <li key={y.value}>
                <Link to={to(yearRange(y.value))} className={pillCls}>
                  {y.value}
                </Link>
              </li>
            ))}
        </Pills>
      </Section>
      <Section title="Try describing a photo">
        <Pills label="Examples">
          {EXAMPLES.map((e) => (
            <li key={e} className="max-w-full">
              <button type="button" className={cn(pillCls, "truncate")} onClick={() => go(e)}>
                <Sparkles />
                <span className="truncate">{e}</span>
              </button>
            </li>
          ))}
        </Pills>
        <p className="mt-2.5 text-sm text-muted">Search matches what is in the photo, and the words in it. It all happens on your own server.</p>
      </Section>
    </motion.div>
  );
}

function IndexingNote({ status }: { status: ReturnType<typeof useProcessing>["status"] }) {
  const reindex = reindexNote(status);
  const note = reindex ?? indexingNote(status);
  const percent = reindex ? (status?.embedding?.percent ?? null) : indexingPercent(status);
  if (!note) return null;
  return (
    <section className="rounded-card bg-surface px-4 py-3" aria-label={reindex ? "Search re-indexing" : "Search indexing"}>
      <p className="text-sm text-muted" role="status">
        {note}
      </p>
      {percent !== null && (
        <div
          className="mt-2.5 h-1.5 overflow-hidden rounded-full bg-line"
          role="progressbar"
          aria-label={reindex ? "Search re-indexing progress" : "Search indexing progress"}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
        >
          <span className="block h-full rounded-full bg-accent transition-[width] duration-300" style={{ width: `${percent}%` }} />
        </div>
      )}
    </section>
  );
}

function Searching() {
  return (
    <p className="flex items-center justify-center gap-2 py-16 text-sm text-muted" role="status">
      <Loader2 className="size-4 motion-safe:animate-spin" aria-hidden="true" />
      Searching…
    </p>
  );
}

/** Nothing to show, or something that went wrong: an icon, what happened, what to do. */
function Notice({ icon: Icon, title, role, children }: { icon: LucideIcon; title: string; role: "status" | "alert"; children?: ReactNode }) {
  return (
    <div role={role} className="mx-auto flex max-w-sm flex-col items-center gap-3 px-2 py-14 text-center">
      <span className="flex size-14 items-center justify-center rounded-full bg-surface text-muted">
        <Icon className="size-6" aria-hidden="true" />
      </span>
      <h2 className="text-[1.0625rem] font-semibold tracking-tight text-balance">{title}</h2>
      {children}
    </div>
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

function Results({ params, onClearFilters }: { params: SearchParams; onClearFilters?: () => void }) {
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
  const [namingPet, setNamingPet] = useState<string[] | null>(null);
  const petsOn = usePetsEnabled() === true;
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
    if (open !== null || confirmed || addIds !== null || shareTarget !== null || namingPet !== null) return;
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
  }, [open, confirmed, addIds, shareTarget, namingPet, sel]);

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
    <Button variant="secondary" onClick={showWeaker}>
      Show weaker matches ({weakerCount.toLocaleString("en-US")})
    </Button>
  );
  const chips = meta.parsed?.chips ?? [];

  if (!started) return <Searching />;

  return (
    <>
      {(indexing || reindexing) && <IndexingNote status={status} />}
      {chips.length > 0 && (
        <ul className="mb-3 flex flex-wrap gap-2" aria-label="Understood from your search">
          {chips.map((c) => (
            <li key={c.kind} className="max-w-full">
              <span
                className={cn(chipCls, "gap-0.5 bg-surface pr-1 pl-3.5")}
                title={`Read “${c.matched}” as ${c.kind === "people" || c.kind === "pets" ? c.kind : `a ${c.kind}`}`}
              >
                <span className="truncate">{c.label}</span>
                <button
                  type="button"
                  aria-label={`Remove ${c.kind} ${c.label}`}
                  onClick={() => removeChip(c)}
                  className="inline-flex size-7 shrink-0 items-center justify-center rounded-full text-muted hover:bg-line/70 hover:text-fg"
                >
                  <X className="size-3.5" aria-hidden="true" />
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
      {noResults && (
        <Notice
          icon={SearchX}
          role="status"
          title={weakerCount > 0 && !weakerShown ? "No close matches" : params.similar ? "No similar photos" : "Nothing matched"}
        >
          <p className="text-[0.9375rem] text-muted text-balance">
            {weakerCount > 0 && !weakerShown
              ? "Nothing in your library matches this closely. There are weaker matches."
              : params.similar
                ? "Nothing else in your library looks like this photo, or the filters leave nothing. Remove a filter to widen it."
                : params.q
                  ? "Try describing the scene with other words, or remove a filter."
                  : "No photo has all of these. Remove a filter to widen the search."}
          </p>
          {indexing && <p className="text-sm text-muted text-balance">Photos that are still being indexed cannot match a description yet.</p>}
          <div className="mt-1 flex flex-wrap justify-center gap-2">
            {weakerCount > 0 && !weakerShown && weakerButton}
            {onClearFilters && (
              <Button variant={weakerCount > 0 && !weakerShown ? "quiet" : "secondary"} onClick={onClearFilters}>
                Clear filters
              </Button>
            )}
          </div>
        </Notice>
      )}
      {notIndexed && (
        <Notice icon={Loader2} role="status" title="This photo is still being indexed">
          <p className="text-[0.9375rem] text-muted text-balance">
            Similar photos can be found once it has been indexed, which usually takes a few minutes after an upload.
          </p>
          <Button variant="primary" className="mt-1" onClick={() => reset()}>
            Try again
          </Button>
        </Notice>
      )}
      {error && !notIndexed && (
        <Notice icon={noModel ? SearchX : Loader2} role="alert" title={noModel ? "Search isn’t available" : "Couldn’t search"}>
          <p className="text-[0.9375rem] text-muted text-balance">
            {noModel
              ? "This server has no search model installed. Filters still work; ask whoever runs the server to install the model."
              : error}
          </p>
          {!noModel && (
            <Button variant="primary" className="mt-1" onClick={() => (items.length ? void loadMore() : reset())}>
              Try again
            </Button>
          )}
        </Notice>
      )}
      {items.length > 0 && (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-x-1 gap-y-1">
            <p className="mr-auto text-sm text-muted" role="status">
              {shown}
            </p>
            <Button variant={sel.selecting ? "secondary" : "quiet"} size="sm" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
              Select
            </Button>
            <SlideshowButton source={() => searchSource(params, "Search results")} />
            {/* The photos of the people or pets filter (a filter-only search: the server can name them all) as an album of their own. */}
            {!meta.ranked && (peopleIdsOf(params.people).length > 0 || params.pets !== undefined || params.pet !== undefined || chips.some((c) => c.kind === "people" || c.kind === "pets" || c.kind === "pet")) && (meta.total ?? 0) > 0 && (
              <Button variant="quiet" size="sm" disabled={busy} onClick={() => setAddIds({ target: { scope, except: [] }, count: meta.total ?? 0, save: true })}>
                Save as album
              </Button>
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
              <Button variant="quiet" size="sm" disabled={sel.count === 0 || busy} onClick={() => void favoriteSelected()}>
                Favorite
              </Button>
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
              <Button
                variant="quiet"
                size="sm"
                disabled={sel.count === 0 || busy}
                onClick={() => setAddIds({ target: sel.target(scope), count: sel.count })}
              >
                Add to album
              </Button>
              <Button
                variant="quiet"
                size="sm"
                disabled={sel.count === 0 || busy}
                onClick={() => setShareTarget(shareTargetOf(sel.target(scope), sel.count))}
              >
                Share
              </Button>
              {petsOn && (
                <Button
                  variant="quiet"
                  size="sm"
                  disabled={sel.count === 0 || busy || sel.all}
                  title={sel.all ? "Choose the photos one by one to name a pet" : undefined}
                  onClick={() => setNamingPet([...sel.ids])}
                >
                  Name pet…
                </Button>
              )}
              <Button
                variant="danger"
                size="sm"
                disabled={sel.count === 0 || busy}
                onClick={() => (sel.all ? setConfirmed(true) : void trashSelected())}
              >
                Move to trash
              </Button>
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
        <div className="mt-6 flex justify-center">{weakerButton}</div>
      )}
      <div ref={sentinel} className="h-1" aria-hidden="true" />
      {loading && items.length > 0 && <p className="py-4 text-center text-sm text-muted" role="status">Loading more…</p>}
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
      {namingPet !== null && (
        <NamePetSheet
          assetIds={namingPet}
          onClose={() => setNamingPet(null)}
          onDone={(r) => {
            setNamingPet(null);
            sel.setSelecting(false);
            show(petNamedToast(r, (id) => navigate(`/pets/${encodeURIComponent(id)}`)));
          }}
        />
      )}
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
