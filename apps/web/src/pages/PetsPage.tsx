import { CheckSquare, FolderPlus, Images, MoreHorizontal, PawPrint, Play, Search, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { api, ApiError, isScope, type AssetListItem, type PetKind, type PetsSummary, type Scope, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmSheet } from "../components/ConfirmSheet";
import { Lightbox } from "../components/Lightbox";
import { Empty, Enter, InlineError, Loading } from "../components/pageKit";
import { RateTools } from "../components/RateTools";
import { SelectionBar } from "../components/SelectionBar";
import { ShareDialog, shareTargetOf, type ShareTarget } from "../components/ShareDialog";
import { TagTools } from "../components/TagTools";
import { TimelineGrid } from "../components/TimelineGrid";
import { useToast } from "../components/Toast";
import { ZipButton } from "../components/ZipButton";
import { offeredKinds, PET_LABEL, PET_TAB, petCount, petKindOf, petPhotos, scanLine, scanning, scanPercent } from "../petsText";
import { useSelection } from "../selection";
import { similarUrl } from "../searchText";
import { PageHeader } from "../shell/PageHeader";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { scopeSource } from "../slideshow/sources";
import { inOrder, runAround } from "../timeline";
import { Button, buttonVariants } from "../ui/Button";
import { Segmented } from "../ui/Controls";
import { Menu, MenuItem } from "../ui/Menu";
import { PAGE, useTimeline } from "../useTimeline";
import { usePetsEnabled } from "../usePets";
import { archiveMessage } from "../viewText";
import { NotFound } from "./NotFound";

/** How often the counts are read again, to notice photos the worker has looked at meanwhile. */
const LIVE_EVERY_MS = 15000;
const THUMB_POLL_MS = 3000;
const THUMB_POLL_MAX = 40; // ticks per photo (about two minutes), then leave it to a reload
/** Photos the lightbox pages through around the current one (it re-centres on every step). */
const LIGHTBOX_RUN = 60;

type Summary = { status: "loading" } | { status: "ready"; data: PetsSummary } | { status: "error"; message: string };

export function PetsPage() {
  const enabled = usePetsEnabled();
  if (enabled === null) {
    return (
      <>
        <PageHeader title="Pets" />
        <Loading>Loading…</Loading>
      </>
    );
  }
  return enabled ? <PetsView /> : <NotFound />;
}

/**
 * Pets: the photos with a dog or a cat as a timeline (the library's own, with selection, bulk actions and the viewer), a switch between
 * All, Dogs and Cats, and, while the worker is still looking through the library, how far it has got.
 */
function PetsView() {
  const { setAnonymous } = useAuth();
  const { toast, show } = useToast();
  const [params, setParams] = useSearchParams();
  const [summary, setSummary] = useState<Summary>({ status: "loading" });
  const [saving, setSaving] = useState<{ target: Target; count: number } | null>(null);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  const load = useCallback(async () => {
    try {
      const data = await api.pets();
      setSummary({ status: "ready", data });
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setSummary((cur) => (cur.status === "ready" ? cur : { status: "error", message: e instanceof Error ? e.message : "Could not load Pets." }));
    }
  }, [setAnonymous]);
  useEffect(() => {
    void load();
  }, [load]);
  // Live: the worker finds pets in the background; read the counts again now and then while the page is open.
  useEffect(() => {
    const t = setInterval(() => {
      if (document.visibilityState === "hidden") return;
      api.pets().then(
        (p) => setSummary((cur) => (cur.status === "ready" && sameCounts(cur.data, p) ? cur : { status: "ready", data: p })),
        () => undefined,
      );
    }, LIVE_EVERY_MS);
    return () => clearInterval(t);
  }, []);

  const data = summary.status === "ready" ? summary.data : null;
  const asked = petKindOf(params.get("kind")) ?? "any";
  const kinds = data ? offeredKinds(data) : (["any"] as PetKind[]);
  // A kind with no photos is not offered: a link to it shows All.
  const kind: PetKind = kinds.includes(asked) ? asked : "any";
  const photos = data ? petCount(data, kind) : 0;

  const header = (
    <PageHeader
      title="Pets"
      actions={
        data && data.any > 0 ? (
          <Menu
            label="Pets"
            trigger={
              <Button size="icon" variant="ghost" aria-label="More">
                <MoreHorizontal />
              </Button>
            }
          >
            <MenuItem onSelect={() => setSaving({ target: { scope: { pets: kind }, except: [] }, count: photos })}>
              <FolderPlus />
              Save as album
            </MenuItem>
            <MenuItem asChild>
              <Link to={`/search?pets=${kind}`}>
                <Search />
                Search with filters
              </Link>
            </MenuItem>
          </Menu>
        ) : undefined
      }
    >
      {/* With one kind, All is that kind: nothing to switch between. */}
      {kinds.length > 2 && (
        <Segmented
          label="Show"
          value={kind}
          onChange={(k) => setParams(k === "any" ? {} : { kind: k }, { replace: true })}
          options={kinds.map((k) => ({ value: k, label: PET_TAB[k] }))}
        />
      )}
    </PageHeader>
  );

  let body: ReactNode;
  if (summary.status === "loading") {
    body = <Loading>Loading…</Loading>;
  } else if (summary.status === "error" || !data) {
    body = (
      <Empty
        icon={TriangleAlert}
        title="Couldn’t load Pets"
        action={
          <Button variant="primary" onClick={() => void load()}>
            Try again
          </Button>
        }
      >
        {summary.status === "error" ? summary.message : ""}
      </Empty>
    );
  } else if (data.any === 0) {
    body = <NoPets summary={data} />;
  } else {
    body = (
      <>
        {scanning(data) && <Progress summary={data} />}
        <PetsPhotos key={kind} kind={kind} photos={photos} failure={failure} show={show} onChanged={load} onSave={(target, count) => setSaving({ target, count })} />
      </>
    );
  }

  // (One header for every state, so it stays put while the answer arrives.)
  return (
    <>
      {header}
      {body}
      {saving !== null && (
        <AddToAlbumDialog
          mode="save"
          ids={saving.target}
          count={saving.count}
          onClose={() => setSaving(null)}
          onDone={(r) => {
            const requested = saving.count;
            setSaving(null);
            show({ message: addedMessage(r, requested) });
          }}
        />
      )}
      {toast}
    </>
  );
}

const sameCounts = (a: PetsSummary, b: PetsSummary) =>
  a.any === b.any && a.scanned === b.scanned && a.total === b.total && a.kinds.every((k, i) => k.photos === b.kinds[i]?.photos && k.kind === b.kinds[i]?.kind);

/** The bar and the line of the backfill: how many of the photos have been looked at. */
function ProgressBar({ summary }: { summary: PetsSummary }) {
  const percent = scanPercent(summary);
  return (
    <div
      className="h-1 overflow-hidden rounded-full bg-line"
      role="progressbar"
      aria-label="Looking for pets"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent ?? undefined}
    >
      <span className="block h-full rounded-full bg-accent transition-[width] duration-500 ease-soft" style={{ width: `${percent ?? 0}%` }} />
    </div>
  );
}

/** Above the photos while the rest of the library is still being looked through. */
function Progress({ summary }: { summary: PetsSummary }) {
  return (
    <section aria-label="Looking for pets" className="mx-4 mt-3 rounded-card bg-surface px-4 py-2.5 md:mx-8">
      <p className="mb-2 truncate text-sm" role="status">
        {scanLine(summary)}. More may turn up.
      </p>
      <ProgressBar summary={summary} />
    </section>
  );
}

/** No photo has a pet (yet): either the worker is still looking, or it found none. */
function NoPets({ summary }: { summary: PetsSummary }) {
  if (scanning(summary)) {
    return (
      <Enter className="mx-auto flex max-w-sm flex-col items-center gap-2 px-4 py-20 text-center">
        <span className="mb-2 flex size-14 items-center justify-center rounded-full bg-surface text-muted">
          <PawPrint className="size-6" strokeWidth={1.6} aria-hidden="true" />
        </span>
        <h2 className="text-[1.0625rem] font-semibold tracking-tight">Looking for dogs and cats</h2>
        <p className="text-[0.9375rem] text-muted">Your library is being looked through. Pets show up here as they are found.</p>
        <div className="mt-3 w-full">
          <ProgressBar summary={summary} />
          <p className="mt-2 text-sm text-muted" role="status">
            {scanLine(summary)}
          </p>
        </div>
      </Enter>
    );
  }
  return (
    <Empty icon={PawPrint} title="No pets found">
      Photos with a dog or a cat appear here once they are in your library.
    </Empty>
  );
}

function PetsPhotos({
  kind,
  photos,
  failure,
  show,
  onChanged,
  onSave,
}: {
  kind: PetKind;
  /** How many photos the kind has now, as the header counts say. */
  photos: number;
  failure: (e: unknown, what: string) => void;
  show: (t: { message: string; tone?: "error"; action?: { label: string; onClick: () => void } }) => void;
  onChanged: () => Promise<void>;
  /** "Save as album": the photos to save (explicit ids, or the whole view) and how many that is. */
  onSave: (target: Target, count: number) => void;
}) {
  const navigate = useNavigate();
  const slideshow = useSlideshow();
  const [open, setOpen] = useState<number | null>(null); // global index of the photo in the lightbox
  const [confirming, setConfirming] = useState<"trash" | null>(null);
  const [busy, setBusy] = useState(false);
  const [addIds, setAddIds] = useState<{ target: Target; count: number } | null>(null);
  const [shareTarget, setShareTarget] = useState<ShareTarget | null>(null);
  const selectedRef = useRef<ReadonlySet<string>>(new Set());
  const tl = useTimeline({ view: { pets: kind }, keep: () => selectedRef.current });
  const { index, loaded, ensure, refresh, remove, patch } = tl;
  const items = useMemo(() => inOrder(loaded), [loaded]);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, index.total);
  const scope = useMemo<Scope>(() => ({ pets: kind }), [kind]);
  useEffect(() => {
    // Selecting everything must not pin every loaded page in memory: only an explicit selection is kept loaded.
    selectedRef.current = sel.all ? new Set() : sel.selected;
  }, [sel.selected, sel.all]);

  // New photos are found in the background: when the count changes, read the timeline again. Selecting would lose the selection to
  // the swap, so that waits until selection mode ends.
  const dirty = useRef(false);
  const selecting = sel.selecting;
  const refreshSoon = useCallback(() => {
    if (selectedRef.current.size > 0 || selecting) dirty.current = true;
    else void refresh();
  }, [refresh, selecting]);
  useEffect(() => {
    if (!selecting && dirty.current) {
      dirty.current = false;
      void refresh();
    }
  }, [selecting, refresh]);
  const seen = useRef(photos);
  useEffect(() => {
    if (seen.current === photos) return;
    seen.current = photos;
    refreshSoon();
  }, [photos, refreshSoon]);

  const refreshItem = useCallback(
    async (id: string) => {
      try {
        const d = await api.getAsset(id);
        patch(new Set([id]), { thumb_url: d.thumb_url, thumbhash: d.thumbhash });
      } catch {
        /* leave the tile as is */
      }
    },
    [patch],
  );
  // Thumbnails appear once the worker is done: look again, lightly, for tiles that still have none.
  const pendingKey = items
    .filter((i) => !i.thumb_url)
    .map((i) => i.id)
    .join(",");
  useEffect(() => {
    if (!pendingKey) return;
    const ticks = new Map<string, number>();
    const t = setInterval(() => {
      for (const id of pendingKey.split(",").slice(0, 12)) {
        const n = (ticks.get(id) ?? 0) + 1;
        ticks.set(id, n);
        if (n <= THUMB_POLL_MAX) void refreshItem(id);
      }
    }, THUMB_POLL_MS);
    return () => clearInterval(t);
  }, [pendingKey, refreshItem]);

  const leaveView = useCallback(
    (ids: string[]) => {
      const left = index.total - remove(new Set(ids)).length;
      setOpen((o) => (o === null || left <= 0 ? null : Math.min(o, left - 1)));
    },
    [index.total, remove],
  );
  const selectedItems = useMemo(() => {
    const ids = new Set(sel.ids);
    return items.filter((i) => ids.has(i.id));
  }, [sel.ids, items]);

  const setFavorite = useCallback(
    async (targets: AssetListItem[], favorite: boolean) => {
      if (targets.length === 0) return;
      const ids = targets.map((t) => t.id);
      const was = targets.filter((t) => t.favorite === true).map((t) => t.id);
      patch(new Set(ids), { favorite });
      try {
        await api.favoriteAssets(ids, favorite);
      } catch (e) {
        patch(new Set(ids), { favorite: false });
        patch(new Set(was), { favorite: true });
        failure(e, favorite ? "Couldn’t add to favorites." : "Couldn’t remove from favorites.");
        return;
      }
      if (targets.length > 1) show({ message: `${targets.length} ${favorite ? "added to" : "removed from"} favorites` });
    },
    [patch, failure, show],
  );
  const favoriteOne = useCallback(
    (id: string, favorite: boolean) => {
      const it = items.find((i) => i.id === id);
      if (it) void setFavorite([it], favorite);
    },
    [items, setFavorite],
  );
  const rateOne = useCallback(
    async (it: AssetListItem, rating: number) => {
      const was = it.rating ?? 0;
      patch(new Set([it.id]), { rating });
      try {
        await api.rateAssets([it.id], rating);
      } catch (e) {
        patch(new Set([it.id]), { rating: was });
        failure(e, "Couldn’t rate.");
      }
    },
    [patch, failure],
  );
  // For the whole view the loaded photos say nothing about the rest: "Favorite" then adds the lot.
  const allSelectedFavorite = !sel.all && selectedItems.length > 0 && selectedItems.every((i) => i.favorite === true);
  const favoriteSelected = useCallback(async () => {
    if (sel.all) {
      const target = sel.target(scope);
      sel.setSelecting(false);
      setBusy(true);
      try {
        const r = await api.favoriteAssets(target, true);
        void refresh();
        show({ message: `${r.count.toLocaleString("en-US")} added to favorites` });
      } catch (e) {
        failure(e, "Couldn’t add to favorites.");
      } finally {
        setBusy(false);
      }
      return;
    }
    const value = !allSelectedFavorite;
    sel.setSelecting(false);
    await setFavorite(selectedItems, value);
  }, [sel, scope, selectedItems, allSelectedFavorite, setFavorite, refresh, show, failure]);

  const undoTrash = useCallback(
    async (target: Target, count: number) => {
      try {
        await api.restoreAssets(target);
        void refresh();
        void onChanged();
        show({ message: `${count.toLocaleString("en-US")} restored` });
      } catch (e) {
        failure(e, "Couldn’t undo. They are still in the trash.");
      }
    },
    [refresh, onChanged, show, failure],
  );
  const trashSelected = useCallback(async () => {
    const target = sel.target(scope);
    const ids = [...sel.ids];
    sel.setSelecting(false);
    setBusy(true);
    try {
      if (!isScope(target)) leaveView(ids);
      const r = await api.trashAssets(target);
      if (isScope(target)) {
        setOpen(null);
        void refresh();
      }
      void onChanged();
      const back: Target = isScope(target) ? (r.batch ? { scope: { trash_batch: r.batch }, except: [] } : []) : target;
      show({
        message: `${r.count.toLocaleString("en-US")} moved to trash`,
        action: { label: "Undo", onClick: () => void undoTrash(back, r.count) },
      });
    } catch (e) {
      void refresh();
      failure(e, "Couldn’t move to trash.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, leaveView, refresh, onChanged, show, failure, undoTrash]);
  const trashOne = useCallback(
    async (it: AssetListItem) => {
      leaveView([it.id]);
      try {
        await api.trashAssets([it.id]);
      } catch (e) {
        void refresh();
        failure(e, "Couldn’t move to trash.");
        return;
      }
      void onChanged();
      show({ message: "1 moved to trash", action: { label: "Undo", onClick: () => void undoTrash([it.id], 1) } });
    },
    [leaveView, refresh, onChanged, show, failure, undoTrash],
  );

  // Archived photos stay in the view (flagged); hidden ones leave it.
  const archiving = sel.all || !selectedItems.every((i) => i.archived === true);
  const archiveSelected = useCallback(async () => {
    const target = sel.target(scope);
    const ids = [...sel.ids];
    sel.setSelecting(false);
    setBusy(true);
    try {
      const r = await api.archiveAssets(target, archiving);
      if (isScope(target)) void refresh();
      else patch(new Set(ids), { archived: archiving });
      show({ message: archiveMessage(r.count, archiving) });
    } catch (e) {
      failure(e, archiving ? "Couldn’t archive." : "Couldn’t restore.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, archiving, refresh, patch, show, failure]);
  const hideSelected = useCallback(async () => {
    const target = sel.target(scope);
    const ids = isScope(target) ? [] : [...sel.ids];
    sel.setSelecting(false);
    if (ids.length > 0) leaveView(ids);
    else setOpen(null);
    setBusy(true);
    try {
      const r = await api.hideAssets(target, true);
      if (ids.length === 0) void refresh();
      void onChanged();
      show({ message: `${r.count.toLocaleString("en-US")} hidden. Find ${r.count === 1 ? "it" : "them"} under Hidden.` });
    } catch (e) {
      void refresh();
      failure(e, "Couldn’t hide.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, leaveView, refresh, onChanged, show, failure]);

  // The lightbox pages through what is loaded around the open photo; keep that stocked as it moves.
  useEffect(() => {
    if (open !== null) ensure(Math.max(0, open - 5), open + PAGE, true);
  }, [open, ensure]);
  const run = open === null ? null : runAround(loaded, open, LIGHTBOX_RUN);

  // Escape clears the selection (then leaves selection mode); the lightbox and the dialogs own the keyboard while they are open.
  useEffect(() => {
    if (open !== null || confirming !== null || addIds !== null || shareTarget !== null) return;
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
  }, [open, confirming, addIds, shareTarget, sel]);

  if (tl.status === "loading") return <Loading>Loading the photos…</Loading>;
  if (tl.status === "error") {
    return (
      <Empty
        icon={TriangleAlert}
        title="Couldn’t load the photos"
        action={
          <Button variant="primary" onClick={tl.retry}>
            Try again
          </Button>
        }
      >
        {tl.error}
      </Empty>
    );
  }
  const quiet = (label: string, onClick: () => void, opts: { disabled?: boolean; aria?: string } = {}) => (
    <Button variant="quiet" size="sm" disabled={opts.disabled} aria-label={opts.aria} onClick={onClick}>
      {label}
    </Button>
  );
  return (
    <>
      {index.total === 0 ? (
        <Empty icon={Images} title="No photos">
          Photos with {kind === "dog" ? "a dog" : kind === "cat" ? "a cat" : "a dog or a cat"} appear here.
        </Empty>
      ) : (
        <div className="mx-auto flex max-w-[96rem] flex-wrap items-center gap-2 px-4 py-3 md:px-8">
          {slideshow && (
            <Button variant="secondary" size="sm" onClick={() => slideshow.start(scopeSource(scope, PET_LABEL[kind]))}>
              <Play />
              Slideshow
            </Button>
          )}
          <Button variant="secondary" size="sm" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
            <CheckSquare />
            Select
          </Button>
        </div>
      )}
      {sel.selecting && (
        <div className="mx-auto max-w-[96rem] px-4 md:px-8">
          <SelectionBar count={sel.count} total={index.total} onSelectAll={sel.selectAll} onClear={sel.clear} onDone={() => sel.setSelecting(false)}>
            {quiet(allSelectedFavorite ? "Unfavorite" : "Favorite", () => void favoriteSelected(), { disabled: sel.count === 0 || busy })}
            <ZipButton
              className={buttonVariants({ variant: "quiet", size: "sm" })}
              target={sel.target(scope)}
              disabled={sel.count === 0}
              onStarted={(message) => show({ message })}
              onError={(e) => failure(e, "Couldn’t download.")}
            />
            <RateTools
              target={sel.target(scope)}
              count={sel.count}
              disabled={sel.count === 0 || busy}
              onDone={(_r, _n, message) => {
                sel.setSelecting(false);
                void refresh();
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
                void refresh();
                show({ message });
              }}
            />
            {quiet(archiving ? "Archive" : "Restore", () => void archiveSelected(), { disabled: sel.count === 0 || busy })}
            {quiet("Hide", () => void hideSelected(), { disabled: sel.count === 0 || busy, aria: "Hide photos" })}
            {quiet("Add to album", () => setAddIds({ target: sel.target(scope), count: sel.count }), { disabled: sel.count === 0 || busy })}
            {quiet("Save as album", () => onSave(sel.target(scope), sel.count), { disabled: sel.count === 0 || busy })}
            {quiet("Share", () => setShareTarget(shareTargetOf(sel.target(scope), sel.count)), { disabled: sel.count === 0 || busy })}
            <Button variant="primary" size="sm" disabled={sel.count === 0 || busy} onClick={() => (sel.all ? setConfirming("trash") : void trashSelected())}>
              Move to trash
            </Button>
          </SelectionBar>
        </div>
      )}
      {index.total > 0 && (
        <TimelineGrid
          index={index}
          loaded={loaded}
          onRange={ensure}
          onOpen={setOpen}
          onThumbError={(id) => void refreshItem(id)}
          selecting={sel.selecting}
          selected={sel.selected}
          onSelect={sel.select}
          onLongPress={sel.begin}
          onFavorite={favoriteOne}
        />
      )}
      {index.total > 0 && (
        <p className="py-4 text-center text-sm text-muted" role="status">
          {petPhotos(index.total)}
        </p>
      )}
      {tl.error && <InlineError onRetry={tl.retry}>{tl.error}</InlineError>}
      {confirming === "trash" && (
        <ConfirmSheet
          title={`Move ${petPhotos(sel.count)} to trash?`}
          confirmLabel="Move to trash"
          onCancel={() => setConfirming(null)}
          onConfirm={() => {
            setConfirming(null);
            void trashSelected();
          }}
        >
          <p>You can restore {sel.count === 1 ? "it" : "them"} from the trash.</p>
        </ConfirmSheet>
      )}
      {addIds !== null && (
        <AddToAlbumDialog
          ids={addIds.target}
          count={addIds.count}
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
      {open !== null && run && (
        <Lightbox
          items={run.items}
          index={open - run.start}
          offset={run.start}
          total={index.total}
          onIndexChange={(rel) => setOpen(run.start + rel)}
          onClose={() => setOpen(null)}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          onTrash={(it) => void trashOne(it)}
          onAddToAlbum={(it) => setAddIds({ target: [it.id], count: 1 })}
          onShare={(it) => setShareTarget({ kind: "photos", ids: [it.id] })}
          onFavorite={(it, favorite) => void setFavorite([it], favorite)}
          onRate={(it, n) => void rateOne(it, n)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
        />
      )}
    </>
  );
}
