import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { NavLink } from "react-router-dom";
import { api, ApiError, isScope, type AssetListItem, type Target } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ProcessingStatus } from "../components/ProcessingStatus";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Lightbox } from "../components/Lightbox";
import { SelectionBar } from "../components/SelectionBar";
import { TimelineGrid } from "../components/TimelineGrid";
import { useToast } from "../components/Toast";
import { useSelection } from "../selection";
import { inOrder, runAround } from "../timeline";
import { PAGE, useTimeline } from "../useTimeline";
import { ACCEPT, filesFromDrop, filterFiles } from "../upload/files";
import { useUploads } from "../upload/UploadProvider";

const REFRESH_EVERY_MS = 3000;
const THUMB_POLL_MS = 3000;
const THUMB_POLL_MAX = 40; // ticks per photo (about two minutes), then leave it to a reload
/** Photos the lightbox pages through around the current one (it re-centres on every step). */
const LIGHTBOX_RUN = 60;

const photos = (n: number) => `${n.toLocaleString("en-US")} photo${n === 1 ? "" : "s"}`;

/** The library as a date-grouped, virtualized timeline; `favorites` limits it to the photos with a heart. */
export function Library({ favorites = false }: { favorites?: boolean }) {
  const { setAnonymous } = useAuth();
  const [open, setOpen] = useState<number | null>(null); // global index of the photo in the lightbox
  const { queue } = useUploads();
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  // Photos waiting in the "add to album" dialog (explicit ids, or the whole view), and a counter that makes the
  // lightbox re-read the photo's albums.
  const [addIds, setAddIds] = useState<{ target: Target; count: number } | null>(null);
  const [albumsVersion, setAlbumsVersion] = useState(0);
  const { toast, show } = useToast();

  const selectedRef = useRef<ReadonlySet<string>>(new Set());
  const tl = useTimeline({ favorite: favorites, keep: () => selectedRef.current });
  const scope = favorites ? "favorites" : "library";
  const { index, loaded, ensure, refresh, remove, patch } = tl;
  const items = useMemo(() => inOrder(loaded), [loaded]);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds, index.total);
  const selectedIds = sel.selected;
  useEffect(() => {
    // Selecting everything must not pin every loaded page in memory: only an explicit selection is kept loaded.
    selectedRef.current = sel.all ? new Set() : selectedIds;
  }, [selectedIds, sel.all]);

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
    if (favorites) return; // a new upload is not a favorite
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
  }, [queue, favorites, refreshSoon]);

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

  const addFiles = useCallback(
    (files: File[]) => {
      const { accepted, skipped } = filterFiles(files);
      if (accepted.length) queue.add(accepted);
      setNotice(
        skipped
          ? `${skipped} file${skipped === 1 ? " was" : "s were"} skipped (not a supported photo type).`
          : null,
      );
    },
    [queue],
  );

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
      void filesFromDrop(e.dataTransfer).then(addFiles);
    },
  };

  const closeLightbox = useCallback(() => setOpen(null), []);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
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
  const trashItems = useCallback(
    async (targets: AssetListItem[]) => {
      if (targets.length === 0) return;
      const ids = targets.map((t) => t.id);
      const left = index.total - remove(new Set(ids)).length;
      // The lightbox shows the photo that took the removed one's place, or closes when nothing is left.
      setOpen((o) => (o === null || left <= 0 ? null : Math.min(o, left - 1)));
      try {
        await api.trashAssets(ids);
      } catch (e) {
        void refresh();
        failure(e, "Couldn’t move to trash.");
        return;
      }
      show({
        message: `${targets.length} moved to trash`,
        action: { label: "Undo", onClick: () => void undoTrash(ids, ids.length) },
      });
    },
    [index.total, remove, refresh, show, failure, undoTrash],
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
    const target = sel.target(scope);
    const ids = new Set(sel.ids);
    sel.setSelecting(false);
    if (isScope(target)) void trashView(target);
    else void trashItems(items.filter((i) => ids.has(i.id)));
  }, [sel, scope, items, trashItems, trashView]);

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
      const target = sel.target(scope);
      sel.setSelecting(false);
      void favoriteView(target, !allSelectedFavorite);
      return;
    }
    const targets = selectedItems;
    const value = !targets.every((i) => i.favorite === true);
    sel.setSelecting(false);
    void setFavorite(targets, value, favorites);
  }, [selectedItems, sel, scope, setFavorite, favoriteView, allSelectedFavorite, favorites]);

  // Trashing the whole view asks first, with the exact number; a few photos go straight to the trash (Undo).
  const onTrashClick = useCallback(() => {
    if (sel.all) setConfirmTrash(true);
    else trashSelected();
  }, [sel.all, trashSelected]);

  // Keyboard: Escape clears the selection (then leaves selection mode); Delete asks, then trashes the selection.
  useEffect(() => {
    if (open !== null || confirmTrash || addIds !== null) return; // the lightbox and the dialog own the keyboard
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest("input, textarea, select, [contenteditable]")) return;
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
  }, [open, confirmTrash, addIds, sel]);

  // The lightbox pages through what is loaded around the open photo; keep that stocked as it moves.
  useEffect(() => {
    if (open !== null) ensure(Math.max(0, open - 5), open + PAGE, true);
  }, [open, ensure]);
  const run = open === null ? null : runAround(loaded, open, LIGHTBOX_RUN);

  if (tl.status === "loading") return <p className="center muted" role="status">Loading your photos…</p>;

  if (tl.status === "error") {
    return (
      <div className="empty" role="alert">
        <h1>Couldn’t load your library</h1>
        <p className="muted">{tl.error}</p>
        <button type="button" className="btn btn-primary" onClick={tl.retry}>
          Try again
        </button>
      </div>
    );
  }

  const tabs = (
    <>
    <nav className="library-tabs" aria-label="Library views">
      <NavLink to="/library" end className="library-tab">
        All photos
      </NavLink>
      <NavLink to="/favorites" end className="library-tab">
        Favorites
      </NavLink>
    </nav>
    <ProcessingStatus />
    </>
  );

  if (index.total === 0) {
    return (
      <div className="library" {...dropProps}>
        {tabs}
        {favorites ? (
          <div className="empty">
            <h1>No favorites yet</h1>
            <p>Tap the heart on a photo, or select photos and choose Favorite, and they will be here.</p>
          </div>
        ) : (
          <div className="empty">
            <h1>Your library is empty</h1>
            <p>Drop photos or a folder anywhere on this page, or pick them from your computer.</p>
            <UploadButtons onFiles={addFiles} />
            {notice && <p className="muted small" role="status">{notice}</p>}
            <p className="muted small">The command-line importer is the better fit for a large existing archive.</p>
          </div>
        )}
        {toast}
        {dragging && <DropOverlay />}
      </div>
    );
  }

  return (
    <div className="library" {...dropProps}>
      <h1 className="sr-only">{favorites ? "Favorites" : "Library"}</h1>
      {tabs}
      <div className="library-bar">
        <UploadButtons onFiles={addFiles} />
        <button
          type="button"
          className="btn btn-quiet"
          aria-pressed={sel.selecting}
          onClick={() => sel.setSelecting(!sel.selecting)}
        >
          Select
        </button>
        {notice && <span className="muted small" role="status">{notice}</span>}
      </div>
      {sel.selecting && (
        <SelectionBar
          count={sel.count}
          total={index.total}
          onSelectAll={sel.selectAll}
          onClear={sel.clear}
          onDone={() => sel.setSelecting(false)}
        >
          <button type="button" className="btn btn-quiet" disabled={sel.count === 0 || bulkBusy} onClick={favoriteSelected}>
            {allSelectedFavorite ? "Unfavorite" : "Favorite"}
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sel.count === 0 || bulkBusy}
            onClick={() => setAddIds({ target: sel.target(scope), count: sel.count })}
          >
            Add to album
          </button>
          <button type="button" className="btn btn-primary" disabled={sel.count === 0 || bulkBusy} onClick={onTrashClick}>
            Move to trash
          </button>
        </SelectionBar>
      )}
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
      <p className="muted center small" role="status">
        {photos(index.total)}
      </p>
      {tl.error && (
        <p className="center" role="alert">
          {tl.error}{" "}
          <button type="button" className="btn btn-quiet" onClick={tl.retry}>
            Retry
          </button>
        </p>
      )}
      {open !== null && run && (
        <Lightbox
          items={run.items}
          index={open - run.start}
          offset={run.start}
          total={index.total}
          onIndexChange={(rel) => setOpen(run.start + rel)}
          onClose={closeLightbox}
          loadDetail={api.getAsset}
          onTrash={(it) => void trashItems([it])}
          onAddToAlbum={(it) => setAddIds({ target: [it.id], count: 1 })}
          onUnpair={(it) => void unpairItem(it)}
          onFavorite={(it, value) => void setFavorite([it], value)}
          refreshKey={albumsVersion}
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
      {toast}
      {dragging && <DropOverlay />}
    </div>
  );
}

function DropOverlay() {
  return (
    <div className="drop-overlay" aria-hidden="true">
      <p>Drop photos or folders to upload</p>
    </div>
  );
}

/** "Upload photos" and "Upload folder" buttons over hidden file inputs. */
function UploadButtons({ onFiles }: { onFiles: (files: File[]) => void }) {
  const files = useRef<HTMLInputElement>(null);
  const folder = useRef<HTMLInputElement>(null);
  const onChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    onFiles(Array.from(e.target.files ?? []));
    e.target.value = ""; // allow choosing the same files again
  };
  return (
    <div className="upload-buttons">
      <button type="button" className="btn btn-primary" onClick={() => files.current?.click()}>
        Upload photos
      </button>
      <button type="button" className="btn btn-quiet" onClick={() => folder.current?.click()}>
        Upload folder
      </button>
      <input ref={files} type="file" multiple accept={ACCEPT} hidden onChange={onChange} aria-label="Choose photos" />
      <input
        ref={folder}
        type="file"
        multiple
        hidden
        onChange={onChange}
        aria-label="Choose a folder"
        {...{ webkitdirectory: "" }}
      />
    </div>
  );
}
