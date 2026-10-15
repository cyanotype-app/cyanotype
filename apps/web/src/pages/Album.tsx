import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, type Album as AlbumData } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { SelectionBar } from "../components/SelectionBar";
import { useToast } from "../components/Toast";
import { dateRange, photoCount } from "../albumText";
import { useSelection } from "../selection";
import { useAssetPages } from "../useAssetPages";

const PAGE = 60;

export function AlbumPage() {
  const { id = "" } = useParams();
  return <AlbumView key={id} id={id} />;
}

/** One album: editable title and description, its photos, selection actions and "Add photos". */
function AlbumView({ id }: { id: string }) {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const [album, setAlbum] = useState<AlbumData | null>(null);
  const [problem, setProblem] = useState<{ status: number; message: string } | null>(null);
  const [editing, setEditing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState<number | null>(null);
  const { toast, show } = useToast();

  const fetchPage = useCallback(
    (cursor: string | null) => api.listAlbumAssets(id, { cursor, limit: PAGE }),
    [id],
  );
  const { items, setItems, done, started, error: pagesError, loadMore, sentinel, refreshItem, reset } =
    useAssetPages(fetchPage);
  const itemIds = useMemo(() => items.map((i) => i.id), [items]);
  const sel = useSelection(itemIds);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  const loadAlbum = useCallback(async () => {
    try {
      setAlbum(await api.getAlbum(id));
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setProblem({ status: e instanceof ApiError ? e.status : 0, message: e instanceof Error ? e.message : "Could not load the album." });
    }
  }, [id, setAnonymous]);
  useEffect(() => {
    void loadAlbum();
  }, [loadAlbum]);

  async function save(patch: { title: string; description: string | null }) {
    setBusy(true);
    try {
      setAlbum(await api.updateAlbum(id, patch));
      setEditing(false);
    } catch (e) {
      failure(e, "Couldn’t save.");
    } finally {
      setBusy(false);
    }
  }

  async function removeSelected() {
    const ids = sel.ids;
    if (ids.length === 0) return;
    setBusy(true);
    try {
      await api.removeFromAlbum(id, ids);
      const gone = new Set(ids);
      if (items.every((i) => gone.has(i.id))) setOpen(null);
      setItems((cur) => cur.filter((i) => !gone.has(i.id)));
      sel.setSelecting(false);
      show({ message: `Removed ${ids.length} from the album` });
      void loadAlbum();
    } catch (e) {
      failure(e, "Couldn’t remove them.");
    } finally {
      setBusy(false);
    }
  }

  async function setCover() {
    const [cover] = sel.ids;
    if (sel.ids.length !== 1 || !cover) return;
    setBusy(true);
    try {
      setAlbum(await api.updateAlbum(id, { cover_asset_id: cover }));
      sel.setSelecting(false);
      show({ message: "Cover updated" });
    } catch (e) {
      failure(e, "Couldn’t set the cover.");
    } finally {
      setBusy(false);
    }
  }

  async function deleteAlbum() {
    setBusy(true);
    try {
      await api.deleteAlbum(id);
      navigate("/albums", { replace: true });
    } catch (e) {
      setConfirmDelete(false);
      setBusy(false);
      failure(e, "Couldn’t delete the album.");
    }
  }

  const onPhotosAdded = useCallback(
    async (count: number) => {
      setAdding(false);
      if (count > 0) {
        show({ message: `Added ${count} to the album` });
        // Newly added photos belong anywhere in the timeline: start the list over.
        reset();
        void loadAlbum();
      } else {
        show({ message: "Those photos are already in the album" });
      }
    },
    [show, reset, loadAlbum],
  );

  // Keyboard: Escape clears the selection, then leaves selection mode.
  useEffect(() => {
    if (open !== null || confirmDelete || adding || editing) return;
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
  }, [open, confirmDelete, adding, editing, sel]);

  const select = useCallback(
    (i: number) => {
      setOpen(i);
      if (i >= items.length - 3 && !done && !pagesError) void loadMore();
    },
    [items.length, done, pagesError, loadMore],
  );
  const closeLightbox = useCallback(() => setOpen(null), []);

  if (problem) {
    return (
      <div className="empty" role="alert">
        <h1>{problem.status === 404 ? "Album not found" : "Couldn’t load the album"}</h1>
        <p className="muted">{problem.status === 404 ? "It may have been deleted." : problem.message}</p>
        <Link to="/albums" className="btn btn-primary">
          All albums
        </Link>
      </div>
    );
  }
  if (!album || !started) {
    return (
      <>
        <p className="center muted" role="status">Loading the album…</p>
        {toast}
      </>
    );
  }

  return (
    <div className="library album-page">
      <p className="small">
        <Link to="/albums">‹ Albums</Link>
      </p>
      {editing ? (
        <AlbumForm album={album} busy={busy} onSave={(p) => void save(p)} onCancel={() => setEditing(false)} />
      ) : (
        <div className="page-head album-head">
          <div>
            <h1>{album.title}</h1>
            {album.description && <p className="album-description">{album.description}</p>}
            <p className="muted small">
              {[photoCount(album.photo_count), dateRange(album.first_at, album.last_at)].filter(Boolean).join(" · ")}
            </p>
          </div>
          <div className="album-head-actions">
            <button type="button" className="btn btn-quiet" onClick={() => setEditing(true)}>
              Edit
            </button>
            <button type="button" className="btn btn-primary" onClick={() => setAdding(true)}>
              Add photos
            </button>
            {items.length > 0 && (
              <button
                type="button"
                className="btn btn-quiet"
                aria-pressed={sel.selecting}
                onClick={() => sel.setSelecting(!sel.selecting)}
              >
                Select
              </button>
            )}
            <button type="button" className="btn btn-quiet btn-danger-quiet" onClick={() => setConfirmDelete(true)}>
              Delete album
            </button>
          </div>
        </div>
      )}
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
            disabled={sel.ids.length !== 1 || busy}
            onClick={() => void setCover()}
          >
            Set as cover
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={sel.ids.length === 0 || busy}
            onClick={() => void removeSelected()}
          >
            Remove from album
          </button>
        </SelectionBar>
      )}
      {items.length === 0 ? (
        <div className="empty">
          <h2>This album is empty</h2>
          <p>Add photos from your library.</p>
        </div>
      ) : (
        <Grid
          items={items}
          onOpen={select}
          onThumbError={(i) => void api.getAsset(i).then((d) => refreshItem(i, { thumb_url: d.thumb_url, thumbhash: d.thumbhash }), () => undefined)}
          selecting={sel.selecting}
          selected={sel.selected}
          onSelect={sel.select}
          onLongPress={sel.begin}
        />
      )}
      <div ref={sentinel} className="sentinel" aria-hidden="true" />
      {pagesError && (
        <p className="center" role="alert">
          {pagesError}{" "}
          <button type="button" className="btn btn-quiet" onClick={() => void loadMore()}>
            Retry
          </button>
        </p>
      )}
      {open !== null && items.length > 0 && (
        <Lightbox
          items={items}
          index={Math.min(open, items.length - 1)}
          onIndexChange={select}
          onClose={closeLightbox}
          loadDetail={api.getAsset}
        />
      )}
      {confirmDelete && (
        <ConfirmDialog
          title="Delete this album?"
          confirmLabel="Delete album"
          danger
          busy={busy}
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void deleteAlbum()}
        >
          <p>The photos stay in your library. Only the album is deleted.</p>
        </ConfirmDialog>
      )}
      {adding && <PhotoPicker albumId={id} onCancel={() => setAdding(false)} onAdded={(n) => void onPhotosAdded(n)} />}
      {toast}
    </div>
  );
}

function AlbumForm({
  album,
  busy,
  onSave,
  onCancel,
}: {
  album: AlbumData;
  busy: boolean;
  onSave: (p: { title: string; description: string | null }) => void;
  onCancel: () => void;
}) {
  const [title, setTitle] = useState(album.title);
  const [description, setDescription] = useState(album.description ?? "");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (title.trim()) onSave({ title: title.trim(), description: description.trim() || null });
  };
  return (
    <form className="album-form" onSubmit={submit}>
      <label htmlFor="album-title-input" className="sr-only">
        Title
      </label>
      <input
        id="album-title-input"
        className="text-input album-title-input"
        value={title}
        maxLength={200}
        autoFocus
        onChange={(e) => setTitle(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
      />
      <label htmlFor="album-desc-input" className="sr-only">
        Description
      </label>
      <textarea
        id="album-desc-input"
        className="text-input"
        rows={3}
        placeholder="Description (optional)"
        value={description}
        maxLength={5000}
        onChange={(e) => setDescription(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
      />
      <div className="album-form-actions">
        <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary" disabled={busy || !title.trim()}>
          Save
        </button>
      </div>
    </form>
  );
}

/** Full-screen picker over the whole library: select photos (same grid and selection bar) and add them. */
function PhotoPicker({
  albumId,
  onCancel,
  onAdded,
}: {
  albumId: string;
  onCancel: () => void;
  onAdded: (count: number) => void;
}) {
  const { setAnonymous } = useAuth();
  const fetchPage = useCallback((cursor: string | null) => api.listAssets({ cursor, limit: PAGE }), []);
  const {
    items: pickItems,
    started: pickStarted,
    error: pickError,
    loadMore: pickMore,
    sentinel: pickSentinel,
  } = useAssetPages(fetchPage);
  const itemIds = useMemo(() => pickItems.map((i) => i.id), [pickItems]);
  const sel = useSelection(itemIds);
  const { setSelecting } = sel;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setSelecting(true), [setSelecting]);
  useEffect(() => {
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.body.style.overflow = overflow;
      document.removeEventListener("keydown", onKey, true);
    };
  }, [onCancel]);

  async function add() {
    setBusy(true);
    setError(null);
    try {
      onAdded((await api.addToAlbum(albumId, sel.ids)).count);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not add the photos.");
      setBusy(false);
    }
  }

  return (
    <div className="picker" role="dialog" aria-modal="true" aria-label="Add photos">
      <div className="picker-head">
        <h2>Add photos</h2>
        <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
      </div>
      <SelectionBar
        count={sel.ids.length}
        total={pickItems.length}
        onSelectAll={sel.selectAll}
        onClear={sel.clear}
      >
        <button
          type="button"
          className="btn btn-primary"
          disabled={sel.ids.length === 0 || busy}
          onClick={() => void add()}
        >
          {sel.ids.length === 0 ? "Add" : `Add ${sel.ids.length}`}
        </button>
      </SelectionBar>
      {error && <p role="alert" className="form-error center">{error}</p>}
      <div className="picker-body">
        {pickStarted && pickItems.length === 0 && <p className="center muted">Your library is empty.</p>}
        <Grid
          items={pickItems}
          onOpen={() => undefined}
          onThumbError={() => undefined}
          selecting
          selected={sel.selected}
          onSelect={sel.select}
        />
        <div ref={pickSentinel} className="sentinel" aria-hidden="true" />
        {pickError && (
          <p className="center" role="alert">
            {pickError}{" "}
            <button type="button" className="btn btn-quiet" onClick={() => void pickMore()}>
              Retry
            </button>
          </p>
        )}
      </div>
    </div>
  );
}
