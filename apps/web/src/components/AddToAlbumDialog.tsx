import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { api, ApiError, type Album } from "../api";
import { useAuth } from "../auth";
import { photoCount } from "../albumText";

interface Props {
  /** The photos to add. */
  ids: readonly string[];
  onClose: () => void;
  /** `count` is how many were new to the album; `created` when the album was made by this dialog. */
  onDone: (result: { album: Album; count: number; created: boolean }) => void;
}

/** Modal to pick an album (with search) or create one inline, then add `ids` to it. */
export function AddToAlbumDialog({ ids, onClose, onDone }: Props) {
  const { setAnonymous } = useAuth();
  const [albums, setAlbums] = useState<Album[] | null>(null);
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    api
      .listAlbums()
      .then((r) => live && setAlbums(r.items))
      .catch((e: unknown) => {
        if (!live) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else setError(e instanceof Error ? e.message : "Could not load your albums.");
        setAlbums([]);
      });
    return () => {
      live = false;
    };
  }, [setAnonymous]);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    return () => prev?.focus?.();
  }, []);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("input")?.focus();
  }, [creating]);

  // Capture so Escape here never also closes the lightbox or clears a selection underneath.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      } else if (e.key === "Tab" && ref.current) {
        const f = ref.current.querySelectorAll<HTMLElement>("button:not(:disabled), input");
        const first = f[0];
        const last = f[f.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last?.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first?.focus();
        }
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (albums ?? []).filter((a) => a.title.toLowerCase().includes(q));
  }, [albums, query]);

  async function run(task: () => Promise<{ album: Album; count: number; created: boolean }>) {
    setBusy(true);
    setError(null);
    try {
      onDone(await task());
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setError(e instanceof Error ? e.message : "Could not add the photos.");
      setBusy(false);
    }
  }

  const addTo = (album: Album) =>
    run(async () => ({ album, count: (await api.addToAlbum(album.id, ids)).count, created: false }));

  const create = (e: FormEvent) => {
    e.preventDefault();
    const t = title.trim();
    if (!t) return;
    void run(async () => {
      const album = await api.createAlbum({ title: t, asset_ids: ids });
      return { album, count: album.photo_count, created: true };
    });
  };

  return (
    <div className="modal-backdrop">
      <div className="modal card album-dialog" role="dialog" aria-modal="true" aria-labelledby="add-album-title" ref={ref}>
        <h2 id="add-album-title">Add {photoCount(ids.length)} to album</h2>
        {creating ? (
          <form onSubmit={create}>
            <label htmlFor="new-album-title" className="sr-only">
              Album title
            </label>
            <input
              id="new-album-title"
              className="text-input"
              placeholder="Album title"
              value={title}
              maxLength={200}
              onChange={(e) => setTitle(e.target.value)}
              disabled={busy}
            />
            {error && <p role="alert" className="form-error">{error}</p>}
            <div className="modal-actions">
              <button type="button" className="btn btn-quiet" onClick={() => setCreating(false)} disabled={busy}>
                Back
              </button>
              <button type="submit" className="btn btn-primary" disabled={busy || !title.trim()}>
                Create and add
              </button>
            </div>
          </form>
        ) : (
          <>
            <label htmlFor="album-search" className="sr-only">
              Search albums
            </label>
            <input
              id="album-search"
              className="text-input"
              type="search"
              placeholder="Search albums"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              disabled={busy}
            />
            {error && <p role="alert" className="form-error">{error}</p>}
            <ul className="album-pick-list" aria-label="Albums">
              {albums === null && <li className="muted small" role="status">Loading…</li>}
              {albums !== null && shown.length === 0 && (
                <li className="muted small">{albums.length === 0 ? "No albums yet." : "No album matches."}</li>
              )}
              {shown.map((a) => (
                <li key={a.id}>
                  <button type="button" className="album-pick" onClick={() => void addTo(a)} disabled={busy}>
                    <span className="album-pick-title">{a.title}</span>
                    <span className="muted small">{photoCount(a.photo_count)}</span>
                  </button>
                </li>
              ))}
            </ul>
            <div className="modal-actions album-dialog-actions">
              <button
                type="button"
                className="btn btn-quiet"
                onClick={() => {
                  setTitle(query.trim());
                  setCreating(true);
                }}
                disabled={busy}
              >
                New album
              </button>
              <button type="button" className="btn btn-quiet" onClick={onClose} disabled={busy}>
                Cancel
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** "Added 3 to Trip", or what happened instead when nothing was new. */
export function addedMessage(r: { album: Album; count: number; created: boolean }, requested: number): string {
  if (r.created) return `Created ${r.album.title} with ${photoCount(r.count)}`;
  if (r.count === 0) return `${requested === 1 ? "That photo is" : "Those photos are"} already in ${r.album.title}`;
  return `Added ${r.count} to ${r.album.title}`;
}
