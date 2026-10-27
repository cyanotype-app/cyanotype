import { CheckSquare, Images, Play, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { api, isScope, type AssetListItem, type LibraryView, type Scope, type Target } from "../api";
import { similarUrl } from "../searchText";
import { useSelection } from "../selection";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { scopeSource } from "../slideshow/sources";
import { inOrder, runAround } from "../timeline";
import { Button, buttonVariants } from "../ui/Button";
import { PAGE, useTimeline } from "../useTimeline";
import { usePetsEnabled } from "../usePets";
import { archiveMessage } from "../viewText";
import { AddToAlbumDialog, addedMessage } from "./AddToAlbumDialog";
import { ConfirmSheet } from "./ConfirmSheet";
import { Lightbox } from "./Lightbox";
import { NamePetSheet, petNamedToast } from "./NamePetSheet";
import { Empty, InlineError, Loading } from "./pageKit";
import { RateTools } from "./RateTools";
import { SelectionBar } from "./SelectionBar";
import { ShareDialog, shareTargetOf, type ShareTarget } from "./ShareDialog";
import { TagTools } from "./TagTools";
import { TimelineGrid } from "./TimelineGrid";
import { ZipButton } from "./ZipButton";

const THUMB_POLL_MS = 3000;
const THUMB_POLL_MAX = 40; // ticks per photo (about two minutes), then leave it to a reload
/** Photos the lightbox pages through around the current one (it re-centres on every step). */
const LIGHTBOX_RUN = 60;

type Toast = (t: { message: string; tone?: "error"; action?: { label: string; onClick: () => void } }) => void;

/** What a page of one person's or one pet's photos can do with a selection beyond the library's own actions: "Not this person" and a cover. */
export interface Membership {
  /** The button's label and the sheet's confirm button: "Not this person". */
  label: string;
  /** The sheet's title: "Not Name?". */
  title: string;
  /** The sheet's text for this many photos. */
  body: (photos: number) => ReactNode;
  /** Take the animals or faces of these photos out; the number that were. */
  remove: (assetIds: string[]) => Promise<number>;
  /** The toast after it. */
  removed: (photos: number) => string;
  /** Make the one selected photo's face or animal the cover (throws an Error whose message is shown). */
  cover: (assetId: string) => Promise<void>;
}

/**
 * The photos of one view (a person, a kind of pet, a named pet) as the library's timeline: month and day headers, the scrubber, "Select all"
 * through the server-side scope, favorites, rating, tags, archive, hide, albums, share, zip, trash with undo, the viewer and the slideshow.
 * The page looks again when its counts change (`changed`: the worker is finding more), so it stays current without a reload.
 */
export function ScopePhotos({
  view,
  scope,
  changed,
  words,
  slideshowTitle,
  empty,
  hiddenWhere,
  membership,
  selectSignal = 0,
  signalLabel,
  failure,
  show,
  onChanged,
  onSave,
}: {
  view: LibraryView;
  scope: Scope;
  /** Anything that changes when the view has new photos (its counts), so the timeline is read again. */
  changed: string;
  /** "3 photos". */
  words: (photos: number) => string;
  slideshowTitle: string;
  /** What the empty view says under "No photos". */
  empty: ReactNode;
  /** Where hidden photos are found, for the toast: "Hidden" or "More, Hidden". */
  hiddenWhere: string;
  membership?: Membership;
  /** A page asking for selection mode (a number that goes up), say from a menu item. */
  selectSignal?: number;
  /** What the selection bar says while nothing is selected, when selection mode was asked for by `selectSignal`. */
  signalLabel?: string;
  failure: (e: unknown, what: string) => void;
  show: Toast;
  onChanged: () => Promise<void>;
  /** "Save as album": the photos to save (explicit ids, or the whole view) and how many that is. */
  onSave: (target: Target, count: number) => void;
}) {
  const navigate = useNavigate();
  const slideshow = useSlideshow();
  const [open, setOpen] = useState<number | null>(null); // global index of the photo in the lightbox
  const [confirming, setConfirming] = useState<"not" | "trash" | null>(null);
  const [busy, setBusy] = useState(false);
  const [addIds, setAddIds] = useState<{ target: Target; count: number } | null>(null);
  const [shareTarget, setShareTarget] = useState<ShareTarget | null>(null);
  // "Name pet…": the photos picked go to a sheet that makes a pet of them (only while Pets is on).
  const petsOn = usePetsEnabled() === true;
  const [namingPet, setNamingPet] = useState<string[] | null>(null);
  const selectedRef = useRef<ReadonlySet<string>>(new Set());
  const tl = useTimeline({ view, keep: () => selectedRef.current });
  const { index, loaded, ensure, refresh, remove, patch } = tl;
  const items = useMemo(() => inOrder(loaded), [loaded]);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, index.total);
  useEffect(() => {
    // Selecting everything must not pin every loaded page in memory: only an explicit selection is kept loaded.
    selectedRef.current = sel.all ? new Set() : sel.selected;
  }, [sel.selected, sel.all]);

  // A page asks for selection mode (the Pets page's "Name a pet").
  const asked = useRef(selectSignal);
  const [byAsk, setByAsk] = useState(false);
  const setSelecting = sel.setSelecting;
  useEffect(() => {
    if (asked.current === selectSignal) return;
    asked.current = selectSignal;
    setSelecting(true);
    setByAsk(true);
  }, [selectSignal, setSelecting]);
  useEffect(() => {
    if (!sel.selecting) setByAsk(false);
  }, [sel.selecting]);

  // New photos are found in the background: when the counts change, read the timeline again. Selecting would lose the
  // selection to the swap, so that waits until selection mode ends.
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
  const seen = useRef(changed);
  useEffect(() => {
    if (seen.current === changed) return;
    seen.current = changed;
    refreshSoon();
  }, [changed, refreshSoon]);

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
      show({ message: `${r.count.toLocaleString("en-US")} hidden. Find ${r.count === 1 ? "it" : "them"} under ${hiddenWhere}.` });
    } catch (e) {
      void refresh();
      failure(e, "Couldn’t hide.");
    } finally {
      setBusy(false);
    }
  }, [sel, scope, leaveView, refresh, onChanged, show, failure, hiddenWhere]);

  async function notThis() {
    if (!membership) return;
    const ids = sel.ids;
    setBusy(true);
    try {
      const removed = await membership.remove(ids);
      setConfirming(null);
      sel.setSelecting(false);
      show({ message: membership.removed(ids.length) });
      if (removed > 0) void refresh();
      void onChanged();
    } catch (e) {
      failure(e, "Couldn’t take the photos out.");
    } finally {
      setBusy(false);
    }
  }
  async function chooseCover() {
    const asset = sel.ids[0];
    if (!asset || !membership) return;
    try {
      await membership.cover(asset);
      sel.setSelecting(false);
      show({ message: "Cover changed" });
    } catch (e) {
      failure(e, "Couldn’t change the cover.");
    }
  }

  // The lightbox pages through what is loaded around the open photo; keep that stocked as it moves.
  useEffect(() => {
    if (open !== null) ensure(Math.max(0, open - 5), open + PAGE, true);
  }, [open, ensure]);
  const run = open === null ? null : runAround(loaded, open, LIGHTBOX_RUN);

  // Escape clears the selection (then leaves selection mode); the lightbox and the dialogs own the keyboard while they are open.
  useEffect(() => {
    if (open !== null || confirming !== null || addIds !== null || shareTarget !== null || namingPet !== null) return;
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
  }, [open, confirming, addIds, shareTarget, namingPet, sel]);

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
  const quiet = (label: string, onClick: () => void, opts: { disabled?: boolean; title?: string; aria?: string } = {}) => (
    <Button variant="quiet" size="sm" disabled={opts.disabled} title={opts.title} aria-label={opts.aria} onClick={onClick}>
      {label}
    </Button>
  );
  return (
    <>
      {index.total === 0 ? (
        <Empty icon={Images} title="No photos">
          {empty}
        </Empty>
      ) : (
        <div className="mx-auto flex max-w-[96rem] flex-wrap items-center gap-2 px-4 py-3 md:px-8">
          {slideshow && (
            <Button variant="secondary" size="sm" onClick={() => slideshow.start(scopeSource(scope, slideshowTitle))}>
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
          <SelectionBar
            count={sel.count}
            total={index.total}
            onSelectAll={sel.selectAll}
            onClear={sel.clear}
            onDone={() => sel.setSelecting(false)}
            emptyLabel={byAsk ? signalLabel : undefined}
          >
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
            {petsOn &&
              quiet("Name pet…", () => setNamingPet([...sel.ids]), {
                disabled: sel.count === 0 || busy || sel.all,
                title: sel.all ? "Choose the photos one by one to name a pet" : undefined,
              })}
            {membership && quiet("Use as cover", () => void chooseCover(), { disabled: sel.count !== 1 })}
            {membership &&
              quiet(membership.label, () => setConfirming("not"), {
                disabled: sel.count === 0 || busy || sel.all,
                title: sel.all ? "Choose the photos one by one to take them out" : undefined,
              })}
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
          {words(index.total)}
        </p>
      )}
      {tl.error && <InlineError onRetry={tl.retry}>{tl.error}</InlineError>}
      {confirming === "not" && membership && (
        <ConfirmSheet title={membership.title} confirmLabel={membership.label} danger busy={busy} onCancel={() => setConfirming(null)} onConfirm={() => void notThis()}>
          {membership.body(sel.count)}
        </ConfirmSheet>
      )}
      {confirming === "trash" && (
        <ConfirmSheet
          title={`Move ${words(sel.count)} to trash?`}
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
      {namingPet !== null && (
        <NamePetSheet
          assetIds={namingPet}
          onClose={() => setNamingPet(null)}
          onDone={(r) => {
            setNamingPet(null);
            sel.setSelecting(false);
            void onChanged();
            show(petNamedToast(r, (petId) => navigate(`/pets/${encodeURIComponent(petId)}`)));
          }}
        />
      )}
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
