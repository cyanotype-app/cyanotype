import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, ApiError, type Album } from "../api";
import { useAuth } from "../auth";
import { AlbumCover } from "../components/AlbumCover";
import { NameDialog } from "../components/NameDialog";
import { SmartAlbumEditor } from "../components/SmartAlbumEditor";
import { blankRules, describeRules, smartCount } from "../smartText";
import { dateRange, hiddenNote, photoCount } from "../albumText";

/** Grid of album cards, most recently updated first. */
export function Albums() {
  const { setAnonymous } = useAuth();
  const navigate = useNavigate();
  const [albums, setAlbums] = useState<Album[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [naming, setNaming] = useState(false);
  const [makingSmart, setMakingSmart] = useState(false);
  const [busy, setBusy] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .listAlbums()
      .then((r) => live && setAlbums(r.items))
      .catch((e: unknown) => {
        if (!live) return;
        if (e instanceof ApiError && e.status === 401) setAnonymous();
        else setError(e instanceof Error ? e.message : "Could not load your albums.");
      });
    return () => {
      live = false;
    };
  }, [setAnonymous]);

  async function create(title: string) {
    setBusy(true);
    setCreateError(null);
    try {
      const a = await api.createAlbum({ title });
      navigate(`/albums/${a.id}`);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setCreateError(e instanceof Error ? e.message : "Could not create the album.");
      setBusy(false);
    }
  }

  async function createSmart(title: string, rules: import("../api").SmartRules) {
    setBusy(true);
    setCreateError(null);
    try {
      const a = await api.createSmartAlbum({ title, rules });
      navigate(`/albums/${a.id}`);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAnonymous();
      else setCreateError(e instanceof Error ? e.message : "Could not create the smart album.");
      setBusy(false);
    }
  }

  return (
    <div className="albums">
      <div className="page-head">
        <h1>Albums</h1>
        <div className="page-head-actions">
          <Link to="/shared" className="btn btn-quiet">
            Shared links
          </Link>
          <button type="button" className="btn btn-quiet" onClick={() => setMakingSmart(true)}>
            New smart album
          </button>
          <button type="button" className="btn btn-primary" onClick={() => setNaming(true)}>
            New album
          </button>
        </div>
      </div>
      {error && (
        <p className="center" role="alert">
          {error}
        </p>
      )}
      {!error && albums === null && <p className="center muted" role="status">Loading your albums…</p>}
      {albums !== null && albums.length === 0 && (
        <div className="empty">
          <h2>No albums yet</h2>
          <p>Group photos into albums. The photos stay in your library either way.</p>
        </div>
      )}
      {albums !== null && albums.length > 0 && (
        <ul className="album-grid" aria-label="Albums">
          {albums.map((a) => (
            <li key={a.id}>
              <Link to={`/albums/${a.id}`} className="album-card">
                <AlbumCover cover={a.cover} />
                <span className="album-card-title">
                  <span className="album-card-name">{a.title}</span>
                  {a.smart && <span className="badge smart-badge">Smart</span>}
                </span>
                <span className="muted small">
                  {[
                    a.smart ? smartCount(a.photo_count, a.photo_count_capped) : photoCount(a.photo_count),
                    hiddenNote(a.hidden_count),
                    dateRange(a.first_at, a.last_at),
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
                {a.smart && a.rules && <span className="muted small album-card-rules">{describeRules(a.rules)}</span>}
              </Link>
            </li>
          ))}
        </ul>
      )}
      {makingSmart && (
        <SmartAlbumEditor
          title="New smart album"
          submitLabel="Create smart album"
          initialRules={blankRules()}
          busy={busy}
          error={createError}
          onSubmit={(t, r) => void createSmart(t, r)}
          onCancel={() => {
            setMakingSmart(false);
            setCreateError(null);
          }}
        />
      )}
      {naming && (
        <NameDialog
          title="New album"
          label="Album title"
          submitLabel="Create"
          busy={busy}
          error={createError}
          onSubmit={(t) => void create(t)}
          onCancel={() => setNaming(false)}
        />
      )}
    </div>
  );
}
