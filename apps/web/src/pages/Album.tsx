import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, isScope, type Album as AlbumData, type AssetListItem } from "../api";
import { useAuth } from "../auth";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Grid } from "../components/Grid";
import { Lightbox } from "../components/Lightbox";
import { SelectionBar } from "../components/SelectionBar";
import { RateTools } from "../components/RateTools";
import { ShareDialog, type ShareTarget } from "../components/ShareDialog";
import { useToast } from "../components/Toast";
import { similarUrl } from "../searchText";
import { dateRange, hiddenNote, photoCount } from "../albumText";
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
  const [sharing, setSharing] = useState<ShareTarget | null>(null);
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
  const sel = useSelection(itemIds, album?.photo_count);

  const failure = useCallback(
    (e: unknown, what: string) => {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else show({ tone: "error", message: `${what} ${e instanceof Error ? e.message : ""}`.trim() });
    },
    [setAnonymous, show],
  );

  // Hearts: shown at once, taken back if the server refuses.
  const setFavorite = useCallback(
    async (target: string, favorite: boolean) => {
      refreshItem(target, { favorite });
      try {
        await api.favoriteAssets([target], favorite);
      } catch (e) {
        refreshItem(target, { favorite: !favorite });
        failure(e, favorite ? "Couldn’t add to favorites." : "Couldn’t remove from favorites.");
      }
    },
    [refreshItem, failure],
  );

  const rateOne = useCallback(
    async (it: { id: string; rating?: number }, rating: number) => {
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
    const count = sel.count;
    if (count === 0) return;
    const target = sel.target({ album: id });
    setBusy(true);
    try {
      await api.removeFromAlbum(id, target);
      if (isScope(target)) {
        // Photos that were never loaded went too: start the list over.
        setOpen(null);
        reset();
      } else {
        const gone = new Set(target);
        if (items.every((i) => gone.has(i.id))) setOpen(null);
        setItems((cur) => cur.filter((i) => !gone.has(i.id)));
      }
      sel.setSelecting(false);
      show({ message: `Removed ${count} from the album` });
      void loadAlbum();
    } catch (e) {
      failure(e, "Couldn’t remove them.");
    } finally {
      setBusy(false);
    }
  }

  async function setCover() {
    const [cover] = sel.ids;
    if (sel.count !== 1 || sel.all || !cover) return;
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

  // Separate a RAW + JPEG pair: the photo loses its RAW badge (the RAW becomes a photo of its own in the library).
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
    if (open !== null || confirmDelete || adding || editing || sharing) return;
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
  }, [open, confirmDelete, adding, editing, sharing, sel]);

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
              {[photoCount(album.photo_count), hiddenNote(album.hidden_count), dateRange(album.first_at, album.last_at)].filter(Boolean).join(" · ")}
            </p>
          </div>
          <div className="album-head-actions">
            <button type="button" className="btn btn-quiet" onClick={() => setEditing(true)}>
              Edit
            </button>
            <button type="button" className="btn btn-primary" onClick={() => setAdding(true)}>
              Add photos
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => setSharing({ kind: "album", albumId: id, albumTitle: album.title })}
            >
              Share
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
          count={sel.count}
          total={album.photo_count}
          onSelectAll={sel.selectAll}
          onClear={sel.clear}
          onDone={() => sel.setSelecting(false)}
        >
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sel.count !== 1 || sel.all || busy}
            onClick={() => void setCover()}
          >
            Set as cover
          </button>
          <RateTools
            target={sel.target({ album: id })}
            count={sel.count}
            disabled={sel.count === 0 || busy}
            onDone={(rating, _n, message) => {
              const picked = sel.all ? null : new Set(sel.ids);
              sel.setSelecting(false);
              if (picked) for (const pid of picked) refreshItem(pid, { rating });
              else reset();
              show({ message });
            }}
            onError={(e) => failure(e, "Couldn’t rate.")}
          />
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sel.ids.length === 0 || busy}
            onClick={() => setSharing({ kind: "photos", ids: sel.ids })}
          >
            Share
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={sel.count === 0 || busy}
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
          onFavorite={(target, favorite) => void setFavorite(target, favorite)}
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
          loadDetails={api.getDetails}
          onShare={(it) => setSharing({ kind: "photos", ids: [it.id] })}
          onFavorite={(it, favorite) => void setFavorite(it.id, favorite)}
          onRate={(it, n) => void rateOne(it, n)}
          onUnpair={(it) => void unpairItem(it)}
          onSimilar={(it) => navigate(similarUrl(it.id))}
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
      {sharing && <ShareDialog target={sharing} onClose={() => setSharing(null)} />}
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
  const [libraryTotal, setLibraryTotal] = useState<number | undefined>(undefined);
  const sel = useSelection(itemIds, libraryTotal);
  const { setSelecting } = sel;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setSelecting(true), [setSelecting]);
  useEffect(() => {
    // "Select all" in the picker means the whole library; the count is the timeline's total.
    let live = true;
    api.timeline().then(
      (t) => live && setLibraryTotal(t.total),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, []);
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
      onAdded((await api.addToAlbum(albumId, sel.target("library"))).count);
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
        count={sel.count}
        total={libraryTotal ?? pickItems.length}
        onSelectAll={sel.selectAll}
        onClear={sel.clear}
      >
        <button
          type="button"
          className="btn btn-primary"
          disabled={sel.count === 0 || busy}
          onClick={() => void add()}
        >
          {sel.count === 0 ? "Add" : `Add ${sel.count.toLocaleString()}`}
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
