import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, type AssetListItem } from "../api";
import { useAuth } from "../auth";
import { AddToAlbumDialog, addedMessage } from "../components/AddToAlbumDialog";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { useSelection } from "../selection";
import { ACCEPT, filesFromDrop, filterFiles } from "../upload/files";
import { useUploads } from "../upload/UploadProvider";

const PAGE = 60;
const THUMB_POLL_MS = 3000;
const THUMB_POLL_MAX = 40; // ticks per photo (about two minutes), then leave it to a reload

const newestFirst = (a: AssetListItem, b: AssetListItem) =>
  Date.parse(b.sort_time) - Date.parse(a.sort_time) || (a.id < b.id ? 1 : a.id > b.id ? -1 : 0);

/** `cur` plus `add`, de-duplicated and back in timeline order (used to put trashed photos back). */
const mergeItems = (cur: AssetListItem[], add: AssetListItem[]) => {
  const have = new Set(cur.map((i) => i.id));
  return [...cur, ...add.filter((i) => !have.has(i.id))].sort(newestFirst);
};

const photos = (n: number) => `${n} photo${n === 1 ? "" : "s"}`;

export function Library() {
  const { setAnonymous } = useAuth();
  const [items, setItems] = useState<AssetListItem[]>([]);
  // Position of the next page: a keyset `cursor`, or (older servers) a `before` timestamp.
  const [next, setNext] = useState<{ cursor?: string; before?: string }>({});
  const [done, setDone] = useState(false); // the last page has been loaded
  const [started, setStarted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const inFlight = useRef(false);
  const sentinel = useRef<HTMLDivElement>(null);
  const { queue } = useUploads();
  const [dragging, setDragging] = useState(false);
  const dragDepth = useRef(0);
  const [notice, setNotice] = useState<string | null>(null);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds);
  const [confirmTrash, setConfirmTrash] = useState(false);
  // Photos waiting in the "add to album" dialog, and a counter that makes the lightbox re-read the photo's albums.
  const [addIds, setAddIds] = useState<string[] | null>(null);
  const [albumsVersion, setAlbumsVersion] = useState(0);
  const { toast, show } = useToast();

  const loadMore = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      const page = await api.listAssets({ ...next, limit: PAGE });
      setItems((cur) => {
        const seen = new Set(cur.map((i) => i.id));
        return [...cur, ...page.items.filter((i) => !seen.has(i.id))];
      });
      if (page.next_cursor) setNext({ cursor: page.next_cursor });
      else if (page.next_before) setNext({ before: page.next_before });
      setDone(!page.next_cursor && !page.next_before);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not load your photos.");
    } finally {
      inFlight.current = false;
      setLoading(false);
      setStarted(true);
    }
  }, [next, setAnonymous]);

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

  // Photos finished by the uploader show up without a reload: re-read the newest page and merge it in.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = queue.onAssetAdded(() => {
      clearTimeout(timer);
      timer = setTimeout(async () => {
        try {
          const page = await api.listAssets({ limit: PAGE });
          setItems((cur) => {
            const have = new Map(cur.map((i) => [i.id, i]));
            const fresh = page.items.filter((i) => !have.has(i.id));
            return fresh.length ? [...cur, ...fresh].sort(newestFirst) : cur;
          });
        } catch {
          /* the next completion (or a reload) will catch up */
        }
      }, 400);
    });
    return () => {
      off();
      clearTimeout(timer);
    };
  }, [queue]);

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

  // Moving to trash is reversible, so there is no confirmation: the photos leave the grid at once (rolled back if
  // the server refuses) and a toast offers Undo, which restores them.
  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );
  const undoTrash = useCallback(
    async (targets: AssetListItem[]) => {
      try {
        await api.restoreAssets(targets.map((t) => t.id));
        setItems((cur) => mergeItems(cur, targets));
        show({ message: `${targets.length} restored` });
      } catch (e) {
        failure(e, "Couldn’t undo. They are still in the trash.");
      }
    },
    [show, failure],
  );
  const trashItems = useCallback(
    async (targets: AssetListItem[]) => {
      if (targets.length === 0) return;
      const gone = new Set(targets.map((t) => t.id));
      if (items.every((i) => gone.has(i.id))) setOpen(null);
      setItems((cur) => cur.filter((i) => !gone.has(i.id)));
      try {
        await api.trashAssets(targets.map((t) => t.id));
      } catch (e) {
        setItems((cur) => mergeItems(cur, targets));
        failure(e, "Couldn’t move to trash.");
        return;
      }
      show({
        message: `${targets.length} moved to trash`,
        action: { label: "Undo", onClick: () => void undoTrash(targets) },
      });
    },
    [items, show, failure, undoTrash],
  );
  const trashSelected = useCallback(() => {
    const ids = new Set(sel.ids);
    sel.setSelecting(false);
    void trashItems(items.filter((i) => ids.has(i.id)));
  }, [sel, items, trashItems]);

  // Keyboard: Escape clears the selection (then leaves selection mode); Delete asks, then trashes the selection.
  useEffect(() => {
    if (open !== null || confirmTrash || addIds !== null) return; // the lightbox and the dialog own the keyboard
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "Escape" && sel.selecting) {
        e.preventDefault();
        sel.escape();
      } else if ((e.key === "Delete" || e.key === "Backspace") && sel.ids.length > 0) {
        e.preventDefault();
        setConfirmTrash(true);
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, confirmTrash, addIds, sel]);

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
      <div className="library" {...dropProps}>
        <div className="empty">
          <h1>Your library is empty</h1>
          <p>Drop photos or a folder anywhere on this page, or pick them from your computer.</p>
          <UploadButtons onFiles={addFiles} />
          {notice && <p className="muted small" role="status">{notice}</p>}
          <p className="muted small">The command-line importer is the better fit for a large existing archive.</p>
        </div>
        {toast}
        {dragging && <DropOverlay />}
      </div>
    );
  }

  return (
    <div className="library" {...dropProps}>
      <h1 className="sr-only">Library</h1>
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
          count={sel.ids.length}
          total={items.length}
          onSelectAll={sel.selectAll}
          onClear={sel.clear}
          onDone={() => sel.setSelecting(false)}
        >
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sel.ids.length === 0}
            onClick={() => setAddIds(sel.ids)}
          >
            Add to album
          </button>
          <button type="button" className="btn btn-primary" disabled={sel.ids.length === 0} onClick={trashSelected}>
            Move to trash
          </button>
        </SelectionBar>
      )}
      <Grid
        items={items}
        onOpen={select}
        onThumbError={(id) => void refreshItem(id)}
        selecting={sel.selecting}
        selected={sel.selected}
        onSelect={sel.select}
        onLongPress={sel.begin}
      />
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
          onTrash={(it) => void trashItems([it])}
          onAddToAlbum={(it) => setAddIds([it.id])}
          refreshKey={albumsVersion}
        />
      )}
      {confirmTrash && (
        <ConfirmDialog
          title={`Move ${photos(sel.ids.length)} to trash?`}
          confirmLabel="Move to trash"
          onCancel={() => setConfirmTrash(false)}
          onConfirm={() => {
            setConfirmTrash(false);
            trashSelected();
          }}
        >
          <p>You can restore {sel.ids.length === 1 ? "it" : "them"} from the trash.</p>
        </ConfirmDialog>
      )}
      {addIds !== null && (
        <AddToAlbumDialog
          ids={addIds}
          onClose={() => setAddIds(null)}
          onDone={(r) => {
            const requested = addIds.length;
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
