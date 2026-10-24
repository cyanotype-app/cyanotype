import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { useParams } from "react-router-dom";
import { ApiError } from "../api";
import { photoCount } from "../albumText";
import { Logo } from "../components/Logo";
import { ZoomControls } from "../components/ZoomControls";
import { fitStyle } from "../photoFit";
import { usePhotoZoom } from "../usePhotoZoom";
import { fileSize } from "../pairText";
import { publicDownloadHref, shareApi, type PublicInfo, type PublicItem } from "../shareApi";
import { expiryText } from "../shareText";
import { placeholderUrl } from "../thumbhash";

type Phase =
  | { kind: "loading" }
  /** Unknown, revoked and expired links all look the same, on purpose. */
  | { kind: "gone" }
  | { kind: "error"; message: string }
  | { kind: "locked" }
  | { kind: "open"; info: PublicInfo };

/** Keep search engines and referrers away from a page whose address is the secret. */
function usePrivatePage(title: string) {
  useEffect(() => {
    const metas = [
      ["robots", "noindex, nofollow, noarchive"],
      ["referrer", "no-referrer"],
    ].map(([name, content]) => {
      const m = document.createElement("meta");
      m.name = name!;
      m.content = content!;
      document.head.appendChild(m);
      return m;
    });
    return () => metas.forEach((m) => m.remove());
  }, []);
  useEffect(() => {
    const prev = document.title;
    document.title = title;
    return () => {
      document.title = prev;
    };
  }, [title]);
}

/** The public page of a share link (`/s/<token>`): no login, a grid, a viewer, downloads when the owner allows them. */
export function ShareView() {
  const { token = "" } = useParams();
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const title = phase.kind === "open" ? (phase.info.title ?? "Shared photos") : "Shared photos";
  usePrivatePage(`${title} · Cyanotype`);

  const load = useCallback(async () => {
    setPhase({ kind: "loading" });
    try {
      const info = await shareApi.publicInfo(token);
      setPhase(info.unlocked ? { kind: "open", info } : { kind: "locked" });
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) setPhase({ kind: "gone" });
      else setPhase({ kind: "error", message: e instanceof Error ? e.message : "Could not load this link." });
    }
  }, [token]);
  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="share-page">
      <header className="share-top">
        <Logo size={22} />
        <span>Cyanotype</span>
      </header>
      {phase.kind === "loading" && <p className="center muted" role="status">Loading…</p>}
      {phase.kind === "gone" && (
        <div className="empty">
          <h1>This link is not available</h1>
          <p>It may have expired or been turned off by its owner.</p>
        </div>
      )}
      {phase.kind === "error" && (
        <div className="empty">
          <h1>Something went wrong</h1>
          <p role="alert">{phase.message}</p>
          <button type="button" className="btn btn-primary" onClick={() => void load()}>
            Try again
          </button>
        </div>
      )}
      {phase.kind === "locked" && <PasswordForm token={token} onUnlocked={() => void load()} />}
      {phase.kind === "open" && <Photos token={token} info={phase.info} onLocked={load} />}
    </div>
  );
}

function PasswordForm({ token, onUnlocked }: { token: string; onUnlocked: () => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!password) return;
    setBusy(true);
    setError(null);
    try {
      await shareApi.publicUnlock(token, password);
      onUnlocked();
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) setError("This link is not available.");
      else if (err instanceof ApiError && err.status === 401) setError("That password is not right.");
      else if (err instanceof ApiError && err.status === 429) setError("Too many attempts. Wait a few minutes and try again.");
      else setError(err instanceof Error ? err.message : "Could not check the password.");
      setBusy(false);
    }
  }

  return (
    <form className="card share-password" onSubmit={(e) => void submit(e)}>
      <h1>This link is protected</h1>
      <label htmlFor="share-password">Password</label>
      <input
        id="share-password"
        className="text-input"
        type="password"
        autoComplete="current-password"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        disabled={busy}
        autoFocus
      />
      {error && (
        <p role="alert" className="form-error">
          {error}
        </p>
      )}
      <button type="submit" className="btn btn-primary" disabled={busy || !password}>
        View photos
      </button>
    </form>
  );
}

function Photos({ token, info, onLocked }: { token: string; info: PublicInfo; onLocked: () => void }) {
  const [items, setItems] = useState<PublicItem[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const loading = useRef(false);
  const lastRefresh = useRef(0);
  const sentinel = useRef<HTMLDivElement>(null);

  const handle = useCallback(
    (e: unknown) => {
      if (e instanceof ApiError && e.status === 401) onLocked();
      else setError(e instanceof Error ? e.message : "Could not load the photos.");
    },
    [onLocked],
  );

  const more = useCallback(
    async (from: string | null) => {
      if (loading.current) return;
      loading.current = true;
      setError(null);
      try {
        const page = await shareApi.publicItems(token, from);
        setItems((prev) => (from ? [...prev, ...page.items.filter((i) => !prev.some((p) => p.id === i.id))] : page.items));
        setCursor(page.next_cursor);
        setLoaded(true);
      } catch (e) {
        handle(e);
      } finally {
        loading.current = false;
      }
    },
    [token, handle],
  );
  useEffect(() => {
    void more(null);
  }, [more]);

  // Presigned image URLs last ten minutes: when one fails, fetch the loaded pages again (at most every 20 s).
  const refresh = useCallback(async () => {
    if (Date.now() - lastRefresh.current < 20_000) return;
    lastRefresh.current = Date.now();
    try {
      let next: string | null = null;
      const fresh: PublicItem[] = [];
      do {
        const page = await shareApi.publicItems(token, next);
        fresh.push(...page.items);
        next = page.next_cursor;
      } while (next && fresh.length < items.length);
      setItems(fresh);
      setCursor(next);
    } catch (e) {
      handle(e);
    }
  }, [token, items.length, handle]);

  useEffect(() => {
    const el = sentinel.current;
    if (!el || !cursor || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) void more(cursor);
    }, { rootMargin: "600px" });
    io.observe(el);
    return () => io.disconnect();
  }, [cursor, more]);

  const update = useCallback((item: PublicItem) => setItems((prev) => prev.map((p) => (p.id === item.id ? item : p))), []);

  return (
    <main className="share-main">
      <div className="share-head">
        <h1>{info.title ?? "Shared photos"}</h1>
        <p className="muted small">
          {[photoCount(info.item_count), info.expires_at ? expiryText(info.expires_at) : null].filter(Boolean).join(" · ")}
        </p>
      </div>
      {loaded && items.length === 0 && !error && (
        <div className="empty">
          <h2>Nothing to see here</h2>
          <p>There are no photos on this link right now.</p>
        </div>
      )}
      {items.length > 0 && (
        <ul className="grid share-grid" aria-label="Photos">
          {items.map((it, i) => (
            <li key={it.id} className="tile-cell">
              <button
                type="button"
                className="tile"
                aria-label={`Open photo ${i + 1} of ${info.item_count}`}
                style={placeholderUrl(it.thumbhash) ? { backgroundImage: `url(${placeholderUrl(it.thumbhash)})` } : undefined}
                onClick={() => setOpen(i)}
              >
                {it.thumb_url ? (
                  <img
                    src={it.thumb_url}
                    alt=""
                    loading="lazy"
                    className="loaded"
                    onError={() => void refresh()}
                  />
                ) : (
                  <span className="tile-pending">Processing…</span>
                )}
              </button>
            </li>
          ))}
        </ul>
      )}
      <div ref={sentinel} className="sentinel" aria-hidden="true" />
      {cursor && typeof IntersectionObserver === "undefined" && (
        <p className="center">
          <button type="button" className="btn btn-quiet" onClick={() => void more(cursor)}>
            Show more
          </button>
        </p>
      )}
      {error && (
        <p className="center" role="alert">
          {error}{" "}
          <button type="button" className="btn btn-quiet" onClick={() => void more(cursor)}>
            Retry
          </button>
        </p>
      )}
      {open !== null && items[open] && (
        <ShareLightbox
          token={token}
          items={items}
          index={Math.min(open, items.length - 1)}
          total={info.item_count}
          allowDownload={info.allow_download}
          onIndexChange={setOpen}
          onClose={() => setOpen(null)}
          onItem={update}
          onImageError={() => void refresh()}
        />
      )}
    </main>
  );
}

/** How often and how long to ask whether the download copy of a JPEG is ready. */
const POLL_MS = 1500;
const POLL_TRIES = 40;

function ShareLightbox({
  token,
  items,
  index,
  total,
  allowDownload,
  onIndexChange,
  onClose,
  onItem,
  onImageError,
}: {
  token: string;
  items: PublicItem[];
  index: number;
  total: number;
  allowDownload: boolean;
  onIndexChange: (i: number) => void;
  onClose: () => void;
  onItem: (item: PublicItem) => void;
  onImageError: () => void;
}) {
  const item = items[index]!;
  const dialogRef = useRef<HTMLDivElement>(null);
  const [loadedId, setLoadedId] = useState<string | null>(null);
  // The size of the picture on show (read when it loads): the CSS fits the photo to the area from its shape.
  const [photoSize, setPhotoSize] = useState<{ id: string; w: number; h: number } | null>(null);
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [gaveUp, setGaveUp] = useState<string | null>(null);
  // Zoom the photo (pinch, ctrl + wheel, double tap, + - z, the buttons). A share link has no sharper picture than the
  // preview (its downloads are attachments, not for display), so zooming scales the preview.
  const stageRef = useRef<HTMLDivElement>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const levelRef = useRef<HTMLSpanElement>(null);
  const announceRef = useRef<HTMLSpanElement>(null);
  const previewLong = photoSize?.id === item.id ? Math.max(photoSize.w, photoSize.h) : 0;
  const zoom = usePhotoZoom({
    stage: stageRef,
    layer: layerRef,
    photo: imgRef,
    level: levelRef,
    announce: announceRef,
    resetKey: item.id,
    active: true,
    previewLong,
    naturalLong: previewLong,
  });
  const go = useCallback(
    (d: number) => {
      const n = index + d;
      if (n >= 0 && n < items.length) onIndexChange(n);
    },
    [index, items.length, onIndexChange],
  );

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (["+", "=", "-", "_", "z"].includes(e.key) && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        if (e.key === "+" || e.key === "=") zoom.zoomIn();
        else if (e.key === "-" || e.key === "_") zoom.zoomOut();
        else if (!e.repeat) zoom.toggle();
      } else if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      } else if (e.key === "Tab" && dialogRef.current) {
        const f = dialogRef.current.querySelectorAll<HTMLElement>("button:not(:disabled), a[href]");
        const first = f[0];
        const last = f[f.length - 1];
        if (!first || !last) return;
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [go, onClose, zoom]);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    dialogRef.current?.querySelector<HTMLElement>("button")?.focus();
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = overflow;
      prev?.focus?.();
    };
  }, []);

  // A JPEG's download is a copy without location that the server makes on first need: ask for it, then wait for it.
  const pending = allowDownload && item.download_pending;
  const itemId = item.id;
  useEffect(() => {
    if (!pending) return;
    let live = true;
    let tries = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      tries++;
      try {
        const fresh = await shareApi.publicItem(token, itemId);
        if (!live) return;
        if (!fresh.download_pending) return onItem(fresh);
      } catch {
        /* try again */
      }
      if (!live) return;
      if (tries >= POLL_TRIES) setGaveUp(itemId);
      else timer = setTimeout(() => void poll(), POLL_MS);
    };
    shareApi.publicPrepare(token, itemId).then(
      () => {
        if (live) timer = setTimeout(() => void poll(), 800);
      },
      () => live && setGaveUp(itemId),
    );
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [pending, token, itemId, onItem]);

  const downloads = allowDownload ? item.downloads : [];
  const src = item.preview_url ?? item.thumb_url;
  const fit = photoSize?.id === item.id ? photoSize : null;
  const bg = placeholderUrl(item.thumbhash);
  const date = new Date(item.sort_time);
  const menuOpen = menuFor === item.id;

  return (
    <div className="lightbox" role="dialog" aria-modal="true" aria-label="Photo" ref={dialogRef}>
      <div className="lightbox-bar">
        <button type="button" className="btn btn-glass" onClick={onClose}>
          Close
        </button>
        <span className="lightbox-count" aria-live="polite">
          {index + 1} / {Math.max(total, items.length)}
        </span>
        <div className="lightbox-right">
          <div className="lightbox-actions">
            {downloads.length === 1 && (
              <a className="btn btn-glass" href={publicDownloadHref(token, item.id, downloads[0]!.id)} rel="noopener">
                Download
              </a>
            )}
            {downloads.length > 1 && (
              <button
                type="button"
                className="btn btn-glass"
                aria-expanded={menuOpen}
                aria-controls="share-downloads"
                onClick={() => setMenuFor(menuOpen ? null : item.id)}
              >
                Download
              </button>
            )}
            {pending && (
              <button type="button" className="btn btn-glass" disabled>
                {gaveUp === item.id ? "Download unavailable" : "Preparing download…"}
              </button>
            )}
          </div>
        </div>
      </div>
      {downloads.length > 1 && menuOpen && (
        <div className="lightbox-downloads" id="share-downloads">
          {downloads.map((d) => (
            <a key={d.id} className="btn btn-glass" href={publicDownloadHref(token, item.id, d.id)} rel="noopener">
              Download {d.label} {d.size_bytes !== null && <span className="muted">{fileSize(d.size_bytes)}</span>}
            </a>
          ))}
        </div>
      )}
      <div
        className={`lightbox-stage${zoom.zoomed ? " zoomed" : ""}`}
        ref={stageRef}
        {...zoom.handlers}
        style={bg ? { backgroundImage: `url(${bg})` } : undefined}
      >
        <button type="button" className="nav nav-prev" aria-label="Previous photo" onClick={() => go(-1)} disabled={index === 0}>
          ‹
        </button>
        {src ? (
          <div className="lightbox-photo" ref={layerRef}>
            <img
              key={item.id}
              ref={imgRef}
              src={src}
              alt=""
              className={`${item.preview_url && loadedId !== item.id ? "loading" : "loaded"}${fit ? " sized" : ""}`}
              style={fitStyle(fit)}
              draggable={false}
              onLoad={(e) => {
                setLoadedId(item.id);
                setPhotoSize({ id: item.id, w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight });
              }}
              onError={onImageError}
            />
          </div>
        ) : (
          <p className="lightbox-msg">This photo is still being processed.</p>
        )}
        <button
          type="button"
          className="nav nav-next"
          aria-label="Next photo"
          onClick={() => go(1)}
          disabled={index >= items.length - 1}
        >
          ›
        </button>
        {src && <ZoomControls zoomed={zoom.zoomed} onIn={zoom.zoomIn} onOut={zoom.zoomOut} onFit={zoom.fit} levelRef={levelRef} announceRef={announceRef} />}
      </div>
      <p className="lightbox-caption">
        <span className="muted">
          {item.width && item.height ? `${item.width} × ${item.height} · ` : ""}
          <time dateTime={item.sort_time}>{Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString()}</time>
        </span>
      </p>
    </div>
  );
}
