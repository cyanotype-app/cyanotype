import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  type LucideIcon,
  Archive,
  ArchiveRestore,
  BookImage,
  Clock,
  Download,
  Eye,
  EyeOff,
  FolderUp,
  Heart,
  ImagePlus,
  Layers,
  Loader2,
  Pencil,
  PawPrint,
  Play,
  Share2,
  SlidersHorizontal,
  Star,
  Tag,
  Trash2,
  Upload,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { dayFromSearch } from "../detailsText";
import { api, ApiError, isScope, type AssetListItem, type StackDetail, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ProcessingStatus } from "../components/ProcessingStatus";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { EmptyState } from "../components/EmptyState";
import { Lightbox } from "../components/Lightbox";
import type { StackView } from "../components/StackBar";
import { usePrefs } from "../usePrefs";
import { archiveMessage, emptyView, pageTitle, type ViewKind } from "../viewText";
import { SelectionBar, type BarAction } from "../components/SelectionBar";
import { RateDialog } from "../components/RateTools";
import { useZip } from "../components/ZipButton";
import { TagDialog } from "../components/TagDialog";
import { EditDialog, type EditKind } from "../components/EditDialog";
import { ShareDialog, shareTargetOf, type ShareTarget } from "../components/ShareDialog";
import { NamePetSheet, petNamedToast } from "../components/NamePetSheet";
import { usePetsEnabled } from "../usePets";
import { MAX_BULK_IDS } from "../api";
import { useHeaderHeight } from "../components/headerHeight";
import { TimelineGrid } from "../components/TimelineGrid";
import { useToast } from "../components/Toast";
import { similarUrl } from "../searchText";
import { useSelection } from "../selection";
import { useUploads } from "../upload/UploadProvider";
import { PageHeader } from "../shell/PageHeader";
import { UploadMenu } from "../shell/AppShell";
import { useUploadPicker } from "../shell/UploadPicker";
import { inOrder, runAround, type GridStyle } from "../timeline";
import { STACK_MESSAGES, alone, canStack, expandIds, expandTarget, hiddenInStacks, positionOf, selectionNote, step, substitute, tileIds } from "../stacks";
import { Button } from "../ui/Button";
import { cn } from "../ui/cn";
import { Menu, MenuItem, MenuLabel, MenuSeparator } from "../ui/Menu";
import { MenuRadioGroup, MenuRadioItem } from "../ui/MenuRadio";
import { Sheet } from "../ui/Sheet";
import { useDesktop } from "../ui/useMedia";
import { PAGE, useTimeline } from "../useTimeline";
import { filesFromDrop } from "../upload/files";
import { useSlideshow } from "../slideshow/SlideshowHost";
import { scopeSource } from "../slideshow/sources";

const REFRESH_EVERY_MS = 3000;
const THUMB_POLL_MS = 3000;
const THUMB_POLL_MAX = 40; // ticks per photo (about two minutes), then leave it to a reload
/** Photos the lightbox pages through around the current one (it re-centres on every step). */
const LIGHTBOX_RUN = 60;

const photos = (n: number) => `${n.toLocaleString("en-US")} photo${n === 1 ? "" : "s"}`;

interface LibraryProps {
  /** Only the photos with a heart (archived ones too). */
  favorites?: boolean;
  /** The Archive view. */
  archive?: boolean;
  /** The Hidden view: the page that shows it has made sure the session is unlocked. */
  hidden?: boolean;
  /** Recently added: the library in upload order, whatever the sort setting. */
  recent?: boolean;
  /** The server said the Hidden view is locked (the ten minutes ran out): the page asks for the password again. */
  onLocked?: () => void;
  /** A slim row under the header (the Hidden view's unlock time and "Lock now"). */
  subbar?: ReactNode;
}

/**
 * The library as a date-grouped, virtualized timeline: all photos (in the order the user chose, remembered per user),
 * favorites, the archive, the hidden photos or the photos by upload time. The order and the grid style are read first so the
 * timeline is laid out once.
 */
export function Library(props: LibraryProps) {
  const plain = !props.favorites && !props.archive && !props.hidden && !props.recent;
  const { prefs, update } = usePrefs();
  if (prefs === null) return <p className="py-24 text-center text-muted" role="status">Loading your photos…</p>;
  const sort = props.recent ? "added" : plain ? prefs.library_sort : "taken";
  // Bursts and stacks as one tile each, or every photo (the Recently added view follows the setting on the server).
  const stacks = plain ? (prefs.stacks_collapsed === false ? "expanded" : "collapsed") : undefined;
  return (
    <LibraryView
      key={`${sort}-${stacks}`}
      {...props}
      sort={sort}
      stacks={stacks}
      onSort={plain ? (s) => void update({ library_sort: s }) : undefined}
      onStacks={plain ? (collapsed) => void update({ stacks_collapsed: collapsed }) : undefined}
    />
  );
}

function LibraryView({
  favorites = false,
  archive = false,
  hidden = false,
  recent = false,
  onLocked,
  subbar,
  sort,
  stacks,
  onSort,
  onStacks,
}: LibraryProps & {
  sort: "taken" | "added";
  stacks?: "collapsed" | "expanded";
  onSort?: (s: "taken" | "added") => void;
  onStacks?: (collapsed: boolean) => void;
}) {
  const kind: ViewKind = hidden ? "hidden" : archive ? "archive" : favorites ? "favorites" : recent ? "recent" : "library";
  const navigate = useNavigate();
  const [search] = useSearchParams();
  const jumpToDay = dayFromSearch(search);
  const { setAnonymous } = useAuth();
  const [open, setOpen] = useState<number | null>(null); // global index of the photo in the lightbox
  const { queue } = useUploads();
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);
  const picker = useUploadPicker();
  const slideshow = useSlideshow();
  const [rateOpen, setRateOpen] = useState(false);
  const [tagOpen, setTagOpen] = useState(false);
  // Rows that keep each photo's shape on a desktop, square crops on a phone.
  const gridStyle: GridStyle = useDesktop() ? "justified" : "square";
  const [density, setDensity] = useDensity(gridStyle);
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  // Photos waiting in the "add to album" dialog (explicit ids, or the whole view), and a counter that makes the
  // lightbox re-read the photo's albums.
  const [addIds, setAddIds] = useState<{ target: Target; count: number } | null>(null);
  // What the share dialog is creating a link for (explicit ids, or the whole view in "select all" mode).
  const [shareTarget, setShareTarget] = useState<ShareTarget | null>(null);
  const [namingPet, setNamingPet] = useState<string[] | null>(null);
  const petsOn = usePetsEnabled() === true;
  const [albumsVersion, setAlbumsVersion] = useState(0);
  const { toast, show } = useToast();
  // "Edit" of the selection: choose what, then the editor (a date shift, one location, one caption for all).
  const [editMenu, setEditMenu] = useState(false);
  const [editKind, setEditKind] = useState<EditKind | null>(null);

  const selectedRef = useRef<ReadonlySet<string>>(new Set());
  const tl = useTimeline({ view: { favorite: favorites, archived: archive, hidden, sort, stacks }, keep: () => selectedRef.current });
  const scope = hidden ? "hidden" : archive ? "archive" : favorites ? "favorites" : "library";
  // The ten minutes of the Hidden view ran out while it was open: back to the password prompt.
  useEffect(() => {
    if (hidden && tl.error === "hidden_locked") onLocked?.();
  }, [hidden, tl.error, onLocked]);
  const { index, loaded, ensure, refresh, remove, patch: patchTimeline } = tl;
  const items = useMemo(() => inOrder(loaded), [loaded]);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, index.total);
  const selectedIds = sel.selected;
  useEffect(() => {
    // Selecting everything must not pin every loaded page in memory: only an explicit selection is kept loaded.
    selectedRef.current = sel.all ? new Set() : selectedIds;
  }, [selectedIds, sel.all]);

  // The stack of the tile on show in the viewer: its photos (read when the tile opens), and the one on show. The timeline holds
  // only the stack's cover, so the viewer swaps the photo on show in for it.
  const tile = open === null ? undefined : loaded.items.get(open);
  const tileId = tile?.id;
  const tileStacked = (tile?.stack?.size ?? 0) > 1;
  const [stackState, setStackState] = useState<{ tile: string; detail: StackDetail; pick: string } | null>(null);
  const [stackVersion, setStackVersion] = useState(0);
  const [stackBusy, setStackBusy] = useState(false);
  useEffect(() => {
    if (!tileId || !tileStacked) return;
    let live = true;
    api.getStack(tileId).then(
      (detail) =>
        live &&
        setStackState((prev) => ({
          tile: tileId,
          detail,
          pick: prev && prev.tile === tileId && detail.items.some((i) => i.id === prev.pick) ? prev.pick : detail.cover_id,
        })),
      () => live && setStackState(null),
    );
    return () => {
      live = false;
    };
  }, [tileId, tileStacked, stackVersion]);
  // A viewer opened again starts at the cover.
  useEffect(() => {
    if (open === null) setStackState(null);
  }, [open]);
  const activeStack = tileStacked && stackState?.tile === tileId ? stackState : null;
  const stackIds = useMemo(() => new Set(activeStack?.detail.items.map((i) => i.id) ?? []), [activeStack]);
  // Fields of loaded photos changed in place (a heart, stars), also in the stack the viewer shows.
  const patch = useCallback(
    (ids: ReadonlySet<string>, fields: Partial<AssetListItem>) => {
      patchTimeline(ids, fields);
      setStackState((s) =>
        s ? { ...s, detail: { ...s.detail, items: s.detail.items.map((i) => (ids.has(i.id) ? { ...i, ...fields } : i)) } } : s,
      );
    },
    [patchTimeline],
  );

  // A tile that stands for a stack stands for all of its photos: bulk calls send them all (a "select all" already means the lot).
  const bulkTarget = useCallback(() => expandTarget(sel.target(scope), items), [sel, scope, items]);
  const hiddenCount = useMemo(() => (sel.all ? 0 : hiddenInStacks(items, selectedIds)), [sel.all, items, selectedIds]);

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

  // Changes that add photos somewhere (uploads, undo, unpair) re-read the counts and the page on screen. Selecting
  // would lose the selection to the swap, so that waits until selection mode ends.
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

  // Photos finished by the uploader show up without a reload.
  useEffect(() => {
    if (favorites || archive || hidden) return; // a new upload is not a favorite, archived or hidden
    let timer: ReturnType<typeof setTimeout> | undefined;
    let last = 0;
    // A big upload finishes photos every second or so: look again shortly after the first, then at most every 3 s.
    const off = queue.onAssetAdded(() => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        last = Date.now();
        refreshSoon();
      }, Math.max(400, last + REFRESH_EVERY_MS - Date.now()));
    });
    return () => {
      off();
      clearTimeout(timer);
    };
  }, [queue, favorites, archive, hidden, refreshSoon]);

  // Thumbnails appear once the worker is done: poll lightly for tiles that still have none.
  const pending = items.filter((i) => !i.thumb_url).map((i) => i.id);
  const pendingKey = pending.join(",");
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

  const isFileDrag = (e: React.DragEvent) => Array.from(e.dataTransfer.types).includes("Files");
  const dropProps = {
    onDragEnter: (e: React.DragEvent) => {
      if (!isFileDrag(e)) return;
      e.preventDefault();
      dragDepth.current++;
      setDragging(true);
    },
    onDragOver: (e: React.DragEvent) => {
      if (isFileDrag(e)) e.preventDefault();
    },
    onDragLeave: (e: React.DragEvent) => {
      if (!isFileDrag(e)) return;
      dragDepth.current = Math.max(0, dragDepth.current - 1);
      if (dragDepth.current === 0) setDragging(false);
    },
    onDrop: (e: React.DragEvent) => {
      if (!isFileDrag(e)) return;
      e.preventDefault();
      dragDepth.current = 0;
      setDragging(false);
      void filesFromDrop(e.dataTransfer).then(picker.add);
    },
  };

  // Changing a stack's cover while the viewer is open must not move the photos under it: the timeline is read again on close.
  const [staleOnClose, setStaleOnClose] = useState(false);
  const closeLightbox = useCallback(() => {
    setOpen(null);
    if (staleOnClose) {
      setStaleOnClose(false);
      void refresh();
    }
  }, [staleOnClose, refresh]);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  // The controls of the stack on show. Taking a photo out or dissolving the stack closes the viewer (the tile it opened from
  // changes); a new cover keeps it open and is shown in the timeline once it closes.
  const changeStack = useCallback(
    async (call: () => Promise<unknown>, message: string, closes: boolean, undo?: () => void) => {
      if (stackBusy) return;
      setStackBusy(true);
      try {
        await call();
      } catch (e) {
        setStackBusy(false);
        failure(e, STACK_MESSAGES.failed);
        return;
      }
      setStackBusy(false);
      if (closes) {
        setOpen(null);
        void refresh();
      } else {
        setStaleOnClose(true);
        setStackVersion((v) => v + 1);
      }
      show({ message, action: undo ? { label: "Undo", onClick: undo } : undefined });
    },
    [stackBusy, failure, refresh, show],
  );

  // Moving to trash is reversible, so there is no confirmation: the photos leave the grid at once (rolled back if
  // the server refuses) and a toast offers Undo, which restores them.
  const undoTrash = useCallback(
    async (target: Target, count: number) => {
      try {
        await api.restoreAssets(target);
        void refresh();
        show({ message: `${count.toLocaleString("en-US")} restored` });
      } catch (e) {
        failure(e, "Couldn’t undo. They are still in the trash.");
      }
    },
    [show, failure, refresh],
  );
  // Photos that are in a stack change the tile that stands for it (a new cover, a smaller count): the timeline is read again
  // after the call instead of being edited in place, and the viewer closes.
  const inStack = useCallback((targets: AssetListItem[]) => targets.some((t) => t.stack || stackIds.has(t.id)), [stackIds]);
  const trashItems = useCallback(
    async (targets: AssetListItem[]) => {
      if (targets.length === 0) return;
      const ids = [...new Set(targets.flatMap(tileIds))];
      const stacked = inStack(targets);
      if (!stacked) {
        const left = index.total - remove(new Set(ids)).length;
        // The lightbox shows the photo that took the removed one's place, or closes when nothing is left.
        setOpen((o) => (o === null || left <= 0 ? null : Math.min(o, left - 1)));
      }
      try {
        await api.trashAssets(ids);
      } catch (e) {
        void refresh();
        failure(e, "Couldn’t move to trash.");
        return;
      }
      if (stacked) {
        setOpen(null);
        void refresh();
      }
      show({
        message: `${ids.length} moved to trash`,
        action: { label: "Undo", onClick: () => void undoTrash(ids, ids.length) },
      });
    },
    [index.total, remove, refresh, show, failure, undoTrash, inStack],
  );
  // "Select all" covers photos that are not loaded, so the server trashes the whole view (minus what was deselected)
  // and the grid is re-read; Undo restores exactly that batch.
  const trashView = useCallback(
    async (target: Target) => {
      setBulkBusy(true);
      try {
        const r = await api.trashAssets(target);
        setOpen(null);
        void refresh();
        const back: Target = r.batch ? { scope: { trash_batch: r.batch }, except: [] } : [];
        show({
          message: `${r.count.toLocaleString("en-US")} moved to trash`,
          action: r.batch ? { label: "Undo", onClick: () => void undoTrash(back, r.count) } : undefined,
        });
      } catch (e) {
        failure(e, "Couldn’t move to trash.");
      } finally {
        setBulkBusy(false);
      }
    },
    [refresh, show, failure, undoTrash],
  );
  const trashSelected = useCallback(() => {
    const target = expandTarget(sel.target(scope), items);
    const ids = new Set(sel.ids);
    sel.setSelecting(false);
    if (isScope(target)) void trashView(target);
    else void trashItems(items.filter((i) => ids.has(i.id)));
  }, [sel, scope, items, trashItems, trashView]);

  // Archive and hide take photos out of this view at once (rolled back by a reload if the server refuses). Archiving can
  // be undone from the toast; hiding cannot (unhiding needs the password, so the Hidden view is where it is undone).
  const leaveView = useCallback(
    (ids: string[]) => {
      const left = index.total - remove(new Set(ids)).length;
      setOpen((o) => (o === null || left <= 0 ? null : Math.min(o, left - 1)));
    },
    [index.total, remove],
  );
  const undoArchive = useCallback(
    async (target: Target, archived: boolean, count: number) => {
      try {
        await api.archiveAssets(target, archived);
        void refresh();
        show({ message: `${count.toLocaleString("en-US")} ${archived ? "archived" : "back in the library"}` });
      } catch (e) {
        failure(e, "Couldn’t undo.");
      }
    },
    [refresh, show, failure],
  );
  const archiveItems = useCallback(
    async (targets: AssetListItem[], archived: boolean) => {
      if (targets.length === 0) return;
      const ids = [...new Set(targets.flatMap(tileIds))];
      const stacked = inStack(targets);
      // In Favorites an archived photo stays (flagged); everywhere else it leaves the view it was archived from or restored to.
      if (favorites) patch(new Set(ids), { archived });
      else if (!stacked) leaveView(ids);
      try {
        await api.archiveAssets(ids, archived);
      } catch (e) {
        void refresh();
        failure(e, archived ? "Couldn’t archive." : "Couldn’t restore.");
        return;
      }
      if (stacked && !favorites) {
        setOpen(null);
        void refresh();
      }
      show({
        message: archiveMessage(ids.length, archived),
        action: { label: "Undo", onClick: () => void undoArchive(ids, !archived, ids.length) },
      });
    },
    [favorites, patch, leaveView, refresh, failure, show, undoArchive, inStack],
  );
  const archiveView = useCallback(
    async (target: Target, archived: boolean) => {
      setBulkBusy(true);
      try {
        const r = await api.archiveAssets(target, archived);
        setOpen(null);
        void refresh();
        const back: Target = r.batch ? { scope: { archive_batch: r.batch }, except: [] } : [];
        show({
          message: archiveMessage(r.count, archived),
          action: r.batch || !archived ? { label: "Undo", onClick: () => void undoArchive(archived ? back : target, !archived, r.count) } : undefined,
        });
      } catch (e) {
        failure(e, archived ? "Couldn’t archive." : "Couldn’t restore.");
      } finally {
        setBulkBusy(false);
      }
    },
    [refresh, show, failure, undoArchive],
  );
  const archiveSelected = useCallback(() => {
    const archived = !archive;
    const target = expandTarget(sel.target(scope), items);
    const ids = new Set(sel.ids);
    sel.setSelecting(false);
    if (isScope(target)) void archiveView(target, archived);
    else void archiveItems(items.filter((i) => ids.has(i.id)), archived);
  }, [archive, sel, scope, items, archiveView, archiveItems]);
  const hideItems = useCallback(
    async (target: Target, ids: string[], hide: boolean, stacked = false) => {
      if (ids.length > 0 && !stacked) leaveView(ids);
      else setOpen(null);
      try {
        const r = await api.hideAssets(target, hide);
        if (ids.length === 0 || stacked) void refresh();
        show({
          message: hide
            ? `${r.count.toLocaleString("en-US")} hidden. Find ${r.count === 1 ? "it" : "them"} under More, Hidden.`
            : `${r.count.toLocaleString("en-US")} back in the library`,
        });
      } catch (e) {
        void refresh();
        failure(e, hide ? "Couldn’t hide." : "Couldn’t unhide.");
      }
    },
    [leaveView, refresh, show, failure],
  );
  const hideSelected = useCallback(() => {
    const target = expandTarget(sel.target(scope), items);
    const ids = isScope(target) ? [] : target.slice();
    const stacked = items.some((i) => sel.selected.has(i.id) && i.stack);
    sel.setSelecting(false);
    void hideItems(target, ids, !hidden, stacked);
  }, [sel, scope, items, hidden, hideItems]);

  // Separate a RAW + JPEG pair: the photo loses its RAW badge and the RAW appears as a photo of its own.
  const unpairItem = useCallback(
    async (it: AssetListItem) => {
      try {
        await api.unpair(it.id);
      } catch (e) {
        failure(e, "Couldn’t unpair.");
        return;
      }
      patch(new Set([it.id]), { paired: [] });
      void refresh();
      show({ message: "Unpaired. They show as separate photos now." });
    },
    [failure, show, patch, refresh],
  );

  // Hearts: shown at once, taken back if the server refuses. In the Favorites view a bulk "Unfavorite" removes the
  // photos; a single heart keeps the tile until the view is reloaded, so an accidental click can be undone.
  const setFavorite = useCallback(
    async (targets: AssetListItem[], favorite: boolean, leave = false) => {
      if (targets.length === 0) return;
      const ids = [...new Set(targets.flatMap(tileIds))];
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
      if (leave && !favorite) {
        const left = index.total - remove(new Set(ids)).length;
        setOpen((o) => (o === null || left <= 0 ? null : Math.min(o, left - 1)));
      }
      if (targets.length > 1 || leave) {
        show({ message: `${targets.length} ${favorite ? "added to" : "removed from"} favorites` });
      }
    },
    [patch, remove, index.total, failure, show],
  );
  const favoriteOne = useCallback(
    (id: string, favorite: boolean) => {
      const it = items.find((i) => i.id === id);
      if (it) void setFavorite([it], favorite);
    },
    [items, setFavorite],
  );
  // Stars: shown at once, taken back if the server refuses.
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
  const selectedItems = useMemo(() => {
    const ids = new Set(sel.ids);
    return items.filter((i) => ids.has(i.id));
  }, [sel.ids, items]);
  // For the whole view the loaded photos say nothing about the rest: in Favorites everything is a favorite, in All
  // photos "Favorite" adds the lot.
  const allSelectedFavorite = sel.all
    ? favorites
    : selectedItems.length > 0 && selectedItems.every((i) => i.favorite === true);
  const favoriteView = useCallback(
    async (target: Target, value: boolean) => {
      setBulkBusy(true);
      try {
        const r = await api.favoriteAssets(target, value);
        void refresh();
        show({ message: `${r.count.toLocaleString("en-US")} ${value ? "added to" : "removed from"} favorites` });
      } catch (e) {
        failure(e, value ? "Couldn’t add to favorites." : "Couldn’t remove from favorites.");
      } finally {
        setBulkBusy(false);
      }
    },
    [refresh, show, failure],
  );
  const favoriteSelected = useCallback(() => {
    if (sel.all) {
      const target = expandTarget(sel.target(scope), items);
      sel.setSelecting(false);
      void favoriteView(target, !allSelectedFavorite);
      return;
    }
    const targets = selectedItems;
    const value = !targets.every((i) => i.favorite === true);
    sel.setSelecting(false);
    void setFavorite(targets, value, favorites);
  }, [selectedItems, items, sel, scope, setFavorite, favoriteView, allSelectedFavorite, favorites]);

  // Trashing the whole view asks first, with the exact number; a few photos go straight to the trash (Undo).
  const onTrashClick = useCallback(() => {
    if (sel.all) setConfirmTrash(true);
    else trashSelected();
  }, [sel.all, trashSelected]);

  // Keyboard: Escape clears the selection (then leaves selection mode); Delete asks, then trashes the selection.
  useEffect(() => {
    if (open !== null || confirmTrash || addIds !== null || shareTarget !== null || namingPet !== null || rateOpen || tagOpen || editMenu || editKind !== null) return; // the lightbox and the dialogs own the keyboard
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest("input, textarea, select, [contenteditable]")) return;
      if (e.defaultPrevented || document.querySelector("[role='menu']")) return; // an open menu takes Escape first
      if (e.key === "Escape" && sel.selecting) {
        e.preventDefault();
        sel.escape();
      } else if ((e.key === "Delete" || e.key === "Backspace") && sel.count > 0) {
        e.preventDefault();
        setConfirmTrash(true);
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, confirmTrash, addIds, shareTarget, namingPet, rateOpen, tagOpen, editMenu, editKind, sel]);

  // Stacking by hand: the selected tiles (a stack's tile brings its photos along) become one stack with the first as its cover.
  const stackSelected = useCallback(async () => {
    // A collapsed stack's tile stands for all of its photos (the first tile's photo comes first and becomes the cover).
    const ids = expandIds(items, sel.ids);
    sel.setSelecting(false);
    try {
      const r = await api.stackAssets(ids);
      void refresh();
      // Undo puts every photo back as it was, also the stacks this one merged.
      show({
        message: STACK_MESSAGES.stacked(r.size),
        action: {
          label: "Undo",
          onClick: () =>
            void api.restoreStacking(r.previous).then(
              () => void refresh(),
              (e: unknown) => failure(e, "Couldn’t undo. The photos are still stacked."),
            ),
        },
      });
    } catch (e) {
      failure(e, STACK_MESSAGES.failed);
    }
  }, [sel, items, refresh, show, failure]);

  // The lightbox pages through what is loaded around the open photo; keep that stocked as it moves.
  useEffect(() => {
    if (open !== null) ensure(Math.max(0, open - 5), open + PAGE, true);
  }, [open, ensure]);
  const run = open === null ? null : runAround(loaded, open, LIGHTBOX_RUN);
  const member = activeStack?.detail.items.find((i) => i.id === activeStack.pick);
  const shown = run && open !== null ? substitute(run.items, open - run.start, member) : [];

  const stackView = activeStack && member ? stackViewOf(activeStack, stackBusy, {
    pick: (id) => setStackState((s) => (s ? { ...s, pick: id } : s)),
    cover: (id) => void changeStack(() => api.setStackCover(id), STACK_MESSAGES.cover, false),
    remove: (id) => void changeStack(() => api.removeFromStack(id), STACK_MESSAGES.removed, true),
    dissolve: (id, restack) =>
      void changeStack(() => api.unstack(id), STACK_MESSAGES.dissolved, true, () => {
        void api.stackAssets(restack).then(
          () => void refresh(),
          (e) => failure(e, STACK_MESSAGES.failed),
        );
      }),
  }) : undefined;

  const zip = useZip({
    target: bulkTarget(),
    onStarted: (message) => show({ message }),
    onError: (e) => failure(e, "Couldn’t download."),
  });
  const pull = usePullToRefresh(open === null && !sel.selecting, tl.refresh);
  const title = pageTitle(kind);

  if (tl.status === "loading") {
    return (
      <div data-library="">
        <PageHeader title={title} />
        <p className="py-24 text-center text-muted" role="status">Loading your photos…</p>
      </div>
    );
  }

  if (tl.status === "error") {
    return (
      <div data-library="">
        <PageHeader title={title} />
        <div role="alert">
          <EmptyState icon={ImagePlus} title="Couldn’t load your library" hint={tl.error ?? "Something went wrong."}>
            <Button variant="primary" onClick={tl.retry}>
              Try again
            </Button>
          </EmptyState>
        </div>
      </div>
    );
  }

  const uploadButtons = (
    <>
      <Button variant="primary" onClick={() => picker.pick("files")}>
        <Upload />
        Upload photos
      </Button>
      <Button variant="secondary" onClick={() => picker.pick("folder")}>
        <FolderUp />
        Upload folder
      </Button>
    </>
  );
  const overlays = (
    <>
      {toast}
      <AnimatePresence>{dragging && <DropOverlay />}</AnimatePresence>
      <PullIndicator pull={pull.distance} refreshing={pull.refreshing} />
    </>
  );
  const notice = picker.notice && (
    <p className="flex items-center gap-2 px-4 py-2 text-sm text-muted md:px-8" role="status">
      <span className="min-w-0 flex-1">{picker.notice}</span>
      <button type="button" className="shrink-0 rounded-full px-2 py-1 text-fg hover:bg-surface" onClick={picker.dismissNotice}>
        Dismiss
      </button>
    </p>
  );

  if (index.total === 0) {
    const e = emptyView(kind);
    return (
      <div data-library="" {...dropProps}>
        <PageHeader title={title} actions={<UploadOnPhones />} />
        {subbar}
        <ProcessingStatus />
        {notice}
        {e ? (
          <EmptyState icon={EMPTY_ICON[kind]} title={e.title} hint={e.body}>
            {kind === "recent" ? (
              <Button variant="primary" onClick={() => picker.pick("files")}>
                <Upload />
                Upload photos
              </Button>
            ) : (
              <Button variant="secondary" asChild>
                <Link to="/photos">Browse photos</Link>
              </Button>
            )}
          </EmptyState>
        ) : (
          <EmptyState
            icon={ImagePlus}
            title="Your library is empty"
            hint="Drop photos or a folder anywhere on this page, or pick them from your device."
            footnote="The command-line importer is the better fit for a large existing archive."
          >
            {uploadButtons}
          </EmptyState>
        )}
        {overlays}
      </div>
    );
  }

  const none = sel.count === 0 || bulkBusy;
  const actions: BarAction[] = [
    {
      id: "favorite",
      label: allSelectedFavorite ? "Unfavorite" : "Favorite",
      icon: <Heart className={allSelectedFavorite ? "fill-current" : undefined} />,
      onSelect: favoriteSelected,
      disabled: none,
    },
    {
      id: "album",
      label: "Album",
      ariaLabel: "Add to album",
      icon: <BookImage />,
      onSelect: () => setAddIds({ target: bulkTarget(), count: sel.count + hiddenCount }),
      disabled: none,
    },
    {
      id: "share",
      label: "Share",
      icon: <Share2 />,
      onSelect: () => setShareTarget(shareTargetOf(bulkTarget(), sel.count + hiddenCount)),
      disabled: none,
    },
    {
      id: "download",
      label: zip.busy ? "Preparing…" : "Download",
      title: "Download the originals as one ZIP",
      icon: <Download />,
      onSelect: () => void zip.start(),
      disabled: sel.count === 0 || zip.busy,
    },
    { id: "tag", label: "Tag", icon: <Tag />, onSelect: () => setTagOpen(true), disabled: none },
    { id: "rate", label: "Rate", icon: <Star />, onSelect: () => setRateOpen(true), disabled: none },
  ];
  if (petsOn) {
    actions.push({
      id: "pet",
      label: "Name pet…",
      icon: <PawPrint />,
      title: sel.all ? "Choose the photos one by one to name a pet" : undefined,
      onSelect: () => {
        const t = bulkTarget();
        if (!isScope(t)) setNamingPet([...t]);
      },
      disabled: none || sel.all,
    });
  }
  if (kind === "library") {
    actions.push({
      id: "stack",
      label: "Stack",
      icon: <Layers />,
      title: STACK_MESSAGES.needTwo,
      onSelect: () => void stackSelected(),
      disabled: bulkBusy || !canStack(items, sel.selected, sel.all),
    });
  }
  if (!hidden) {
    actions.push({
      id: "archive",
      label: archive ? "Restore" : "Archive",
      icon: archive ? <ArchiveRestore /> : <Archive />,
      onSelect: archiveSelected,
      disabled: none,
    });
  }
  actions.push(
    { id: "hide", label: hidden ? "Unhide" : "Hide", icon: hidden ? <Eye /> : <EyeOff />, onSelect: hideSelected, disabled: none },
    {
      id: "edit",
      label: "Edit",
      icon: <Pencil />,
      title: sel.count > MAX_BULK_IDS ? `Edit at most ${MAX_BULK_IDS} photos at a time` : undefined,
      onSelect: () => setEditMenu(true),
      disabled: none || sel.count > MAX_BULK_IDS,
    },
    { id: "trash", label: "Trash", ariaLabel: "Move to trash", icon: <Trash2 />, onSelect: onTrashClick, disabled: none, danger: true, pinned: true },
  );

  const canSlideshow = kind === "library" || kind === "favorites" || kind === "archive";
  return (
    <div data-library="" {...dropProps}>
      <PageHeader
        title={title}
        actions={
          <>
            <Button variant="ghost" size="sm" aria-pressed={sel.selecting} onClick={() => sel.setSelecting(!sel.selecting)}>
              Select
            </Button>
            <Menu
              label="View options"
              trigger={
                <Button variant="ghost" size="icon" aria-label="View options">
                  <SlidersHorizontal />
                </Button>
              }
            >
              <MenuLabel>Size</MenuLabel>
              <MenuItem onSelect={() => setDensity(-1)}>
                <ZoomIn />
                Larger photos
              </MenuItem>
              <MenuItem onSelect={() => setDensity(1)}>
                <ZoomOut />
                Smaller photos
              </MenuItem>
              {onSort && (
                <>
                  <MenuSeparator />
                  <MenuLabel>Sort by</MenuLabel>
                  <MenuRadioGroup label="Sort photos by" value={sort} onValueChange={onSort}>
                    <MenuRadioItem value="taken">Date taken</MenuRadioItem>
                    <MenuRadioItem value="added">Date added</MenuRadioItem>
                  </MenuRadioGroup>
                </>
              )}
              {onStacks && (
                <>
                  <MenuSeparator />
                  <MenuLabel>Stacks</MenuLabel>
                  <MenuRadioGroup
                    label="Stacks"
                    value={stacks === "expanded" ? "expanded" : "collapsed"}
                    onValueChange={(v) => onStacks(v === "collapsed")}
                  >
                    <MenuRadioItem value="collapsed">Stacked</MenuRadioItem>
                    <MenuRadioItem value="expanded">Every photo</MenuRadioItem>
                  </MenuRadioGroup>
                </>
              )}
              {slideshow && canSlideshow && (
                <>
                  <MenuSeparator />
                  <MenuItem onSelect={() => slideshow.start(scopeSource(scope, title))}>
                    <Play />
                    Slideshow
                  </MenuItem>
                </>
              )}
            </Menu>
            <UploadOnPhones />
          </>
        }
      />
      {subbar}
      <ProcessingStatus />
      {notice}
      <AnimatePresence>
        {sel.selecting && (
          <SelectionBar
            key="selection"
            count={sel.count}
            total={index.total}
            onSelectAll={sel.selectAll}
            onClear={sel.clear}
            onDone={() => sel.setSelecting(false)}
            note={selectionNote(hiddenCount)}
            actions={actions}
          />
        )}
      </AnimatePresence>
      <TimelineGrid
        index={index}
        loaded={loaded}
        onRange={ensure}
        onOpen={setOpen}
        onThumbError={(id) => void refreshItem(id)}
        selecting={sel.selecting}
        selected={sel.selected}
        onSelect={sel.select}
        onLongPress={sel.dragStart}
        onDragOver={sel.dragOver}
        onDragEnd={sel.dragEnd}
        onFavorite={favoriteOne}
        jumpToDay={jumpToDay}
        gridStyle={gridStyle}
        density={density}
        onDensity={setDensity}
      />
      <p className="py-8 text-center text-sm text-muted" role="status">
        {photos(index.total)}
      </p>
      {tl.error && (
        <p className="flex items-center justify-center gap-2 pb-6 text-center" role="alert">
          {tl.error}
          <Button variant="secondary" size="sm" onClick={tl.retry}>
            Retry
          </Button>
        </p>
      )}
      {open !== null && run && (
        <Lightbox
          items={shown}
          index={open - run.start}
          offset={run.start}
          total={index.total}
          onIndexChange={(rel) => setOpen(run.start + rel)}
          onClose={closeLightbox}
          loadDetail={api.getAsset}
          loadDetails={api.getDetails}
          stack={stackView}
          onTrash={(it) => void trashItems([alone(it)])}
          onAddToAlbum={(it) => setAddIds({ target: [it.id], count: 1 })}
          onShare={(it) => setShareTarget({ kind: "photos", ids: [it.id] })}
          onUnpair={(it) => void unpairItem(it)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
          onFavorite={(it, value) => void setFavorite([alone(it)], value)}
          onRate={(it, n) => void rateOne(it, n)}
          onArchive={hidden ? undefined : (it) => void archiveItems([alone(it)], it.archived !== true)}
          onHide={(it) => void hideItems([it.id], [it.id], !hidden, inStack([alone(it)]))}
          refreshKey={albumsVersion}
          onEdited={() => void refresh()}
        />
      )}
      {confirmTrash && (
        <ConfirmDialog
          title={`Move ${photos(sel.count)} to trash?`}
          confirmLabel="Move to trash"
          onCancel={() => setConfirmTrash(false)}
          onConfirm={() => {
            setConfirmTrash(false);
            trashSelected();
          }}
        >
          <p>You can restore {sel.count === 1 ? "it" : "them"} from the trash.</p>
        </ConfirmDialog>
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
            setAlbumsVersion((v) => v + 1);
            show({ message: addedMessage(r, requested) });
          }}
        />
      )}
      <Sheet
        open={editMenu}
        onOpenChange={setEditMenu}
        size="sm"
        title="Edit photos"
        description={photos(sel.count)}
        footer={
          <Button variant="secondary" onClick={() => setEditMenu(false)}>
            Cancel
          </Button>
        }
      >
        <div className="flex flex-col gap-1">
          {([["date", "Adjust date and time"], ["location", "Set location"], ["caption", "Add a caption"]] as const).map(([k, l]) => (
            <button
              key={k}
              type="button"
              className="flex min-h-12 items-center rounded-xl px-3 text-left text-[0.9375rem] hover:bg-surface"
              onClick={() => {
                setEditMenu(false);
                setEditKind(k);
              }}
            >
              {l}
            </button>
          ))}
        </div>
      </Sheet>
      {editKind !== null && (
        <EditDialog
          kind={editKind}
          target={bulkTarget()}
          count={sel.count}
          onClose={() => setEditKind(null)}
          onDone={(r) => {
            setEditKind(null);
            sel.setSelecting(false);
            void refresh();
            show({ message: `${r.updated.toLocaleString("en-US")} ${r.updated === 1 ? "photo" : "photos"} updated` });
          }}
        />
      )}
      <RateDialog
        open={rateOpen}
        onOpenChange={setRateOpen}
        target={bulkTarget()}
        count={sel.count}
        onDone={(_r, _n, message) => {
          sel.setSelecting(false);
          void refresh();
          show({ message });
        }}
        onError={(e) => failure(e, "Couldn’t rate.")}
      />
      {tagOpen && (
        <TagDialog
          target={bulkTarget()}
          count={sel.count}
          onClose={() => setTagOpen(false)}
          onDone={(r) => {
            setTagOpen(false);
            sel.setSelecting(false);
            void refresh();
            show({ message: r.message });
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
      {overlays}
    </div>
  );
}

interface StackActions {
  pick: (id: string) => void;
  cover: (id: string) => void;
  remove: (id: string) => void;
  /** `restack`: the stack's photos with the cover first (what Undo stacks again). */
  dissolve: (id: string, restack: string[]) => void;
}

/** What the viewer's stack strip shows and does for the stack on show (`pick` is the photo on show). */
function stackViewOf(stack: { detail: StackDetail; pick: string }, busy: boolean, act: StackActions): StackView {
  const { detail, pick } = stack;
  const position = positionOf(detail.items, pick);
  const restack = detail.items.map((i) => i.id).sort((a, b) => Number(b === detail.cover_id) - Number(a === detail.cover_id));
  return {
    items: detail.items,
    position,
    coverId: detail.cover_id,
    manual: detail.manual,
    busy,
    onPick: act.pick,
    onStep: (delta) => {
      const next = detail.items[step(position, delta, detail.items.length)];
      if (next) act.pick(next.id);
    },
    onCover: () => act.cover(pick),
    onRemove: () => act.remove(pick),
    onDissolve: () => act.dissolve(pick, restack),
  };
}

const EMPTY_ICON: Record<ViewKind, LucideIcon> = {
  library: ImagePlus,
  favorites: Heart,
  recent: Clock,
  archive: Archive,
  hidden: EyeOff,
};

/** The header's Upload menu, on phones only (the sidebar has it on a desktop). */
function UploadOnPhones() {
  return (
    <div className="md:hidden">
      <UploadMenu compact />
    </div>
  );
}

/** Shown while files are dragged over the page. */
function DropOverlay() {
  const reduce = useReducedMotion();
  return (
    <motion.div
      className="pointer-events-none fixed inset-0 z-50 grid grid-cols-1 place-items-center bg-scrim backdrop-blur-sm"
      aria-hidden="true"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: reduce ? 0 : 0.15 }}
      data-drop-overlay=""
    >
      <div className="flex flex-col items-center gap-3 rounded-sheet border-2 border-dashed border-white/70 px-10 py-9 text-white">
        <Upload className="size-8" />
        <p className="text-lg font-medium">Drop photos or folders to upload</p>
      </div>
    </motion.div>
  );
}

const DENSITY_KEY = "cyanotype.grid-density";
const DENSITY_MIN = -5;
const DENSITY_MAX = 6;

/** How many columns more (or smaller rows) than usual, per grid style, kept in this browser (a pinch or the menu changes it). */
function useDensity(style: GridStyle): [number, (delta: number) => void] {
  const [all, setAll] = useState<Record<GridStyle, number>>(() => {
    try {
      const v = JSON.parse(localStorage.getItem(DENSITY_KEY) ?? "{}") as Partial<Record<GridStyle, number>>;
      return { square: Number(v.square) || 0, justified: Number(v.justified) || 0 };
    } catch {
      return { square: 0, justified: 0 };
    }
  });
  const change = useCallback(
    (delta: number) =>
      setAll((a) => {
        const next = { ...a, [style]: Math.min(DENSITY_MAX, Math.max(DENSITY_MIN, a[style] + delta)) };
        try {
          localStorage.setItem(DENSITY_KEY, JSON.stringify(next));
        } catch {
          /* private mode: the change lasts until the page is closed */
        }
        return next;
      }),
    [style],
  );
  return [all[style], change];
}

/** Finger travel before a pull starts, the distance (px of pull) that refreshes, and the furthest the spinner is drawn down. */
const PULL_START = 10;
const PULL_AT = 64;
const PULL_MAX = 96;

/**
 * Pull down at the top of the page (touch) to read the timeline again. The page's own overscroll is cancelled while pulling, so
 * the browser's reload gesture (Android) and the rubber band (iOS) do not also fire; with two fingers (a pinch), a drag that
 * starts on the date rail, or anything not at the very top, nothing happens.
 */
function usePullToRefresh(enabled: boolean, onRefresh: () => Promise<void>): { distance: number; refreshing: boolean } {
  const [distance, setDistance] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const live = useRef({ enabled, onRefresh, refreshing });
  useEffect(() => {
    live.current = { enabled, onRefresh, refreshing };
  });
  useEffect(() => {
    let from = 0;
    let active = false;
    let pulled = 0;
    const start = (e: TouchEvent) => {
      active = false;
      pulled = 0;
      const t = e.target as HTMLElement | null;
      if (!live.current.enabled || live.current.refreshing || e.touches.length !== 1 || window.scrollY > 0) return;
      if (t?.closest("[data-handle], dialog, [role='menu'], input, textarea")) return;
      from = e.touches[0]!.clientY;
      active = true;
    };
    const move = (e: TouchEvent) => {
      if (!active) return;
      const dy = e.touches[0] ? e.touches[0].clientY - from : 0;
      if (e.touches.length !== 1 || window.scrollY > 0 || dy < 0) {
        active = false;
        pulled = 0;
        setDistance(0);
        return;
      }
      if (dy < PULL_START) return;
      if (e.cancelable) e.preventDefault();
      pulled = Math.min(PULL_MAX, (dy - PULL_START) * 0.55);
      setDistance(pulled);
    };
    const end = () => {
      const go = active && pulled >= PULL_AT * 0.8;
      active = false;
      if (!go) {
        setDistance(0);
        return;
      }
      setRefreshing(true);
      setDistance(PULL_AT * 0.7);
      void Promise.all([live.current.onRefresh(), new Promise((r) => setTimeout(r, 600))]).finally(() => {
        setRefreshing(false);
        setDistance(0);
      });
    };
    document.addEventListener("touchstart", start, { passive: true });
    document.addEventListener("touchmove", move, { passive: false });
    document.addEventListener("touchend", end);
    document.addEventListener("touchcancel", end);
    return () => {
      document.removeEventListener("touchstart", start);
      document.removeEventListener("touchmove", move);
      document.removeEventListener("touchend", end);
      document.removeEventListener("touchcancel", end);
    };
  }, []);
  return { distance, refreshing };
}

/** The small spinner that follows the pull, under the page's header. */
function PullIndicator({ pull, refreshing }: { pull: number; refreshing: boolean }) {
  const hdr = useHeaderHeight();
  if (pull <= 0 && !refreshing) return null;
  return (
    <div className="pointer-events-none fixed inset-x-0 z-[7] flex justify-center md:hidden" style={{ top: hdr }} aria-hidden={!refreshing}>
      <div
        className="grid size-9 grid-cols-1 place-items-center rounded-full bg-card text-accent shadow-float"
        style={{ transform: `translateY(${pull - 24}px) scale(${Math.min(1, 0.4 + pull / PULL_AT)})`, opacity: Math.min(1, pull / 30) }}
      >
        <Loader2
          className={cn("size-5", refreshing && "animate-spin")}
          style={refreshing ? undefined : { transform: `rotate(${pull * 4}deg)` }}
        />
        {refreshing && (
          <span className="sr-only" role="status">
            Refreshing
          </span>
        )}
      </div>
    </div>
  );
}
