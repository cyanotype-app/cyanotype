import { Link } from "react-router-dom";
import { useCallback, useEffect, useRef, useState } from "react";
import { downloadHref, type AssetDetail, type AssetListItem, type PhotoDetails } from "../api";
import { useInfoHistory, useVerticalSwipe } from "../useInfoPanel";
import { DetailsPanel, type DetailsState } from "./DetailsPanel";
import { placeLine } from "../map/geo";
import { fileSize, pairLabel, primaryFormat } from "../pairText";
import { placeholderUrl } from "../thumbhash";
import { HeartIcon } from "./Tile";

interface Props {
  items: AssetListItem[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  loadDetail: (id: string) => Promise<AssetDetail>;
  /** Read the photo's details (the Info panel: date, place, camera, exposure, files). The Info button shows when given. */
  loadDetails?: (id: string) => Promise<PhotoDetails>;
  /** Move the current photo to the trash. The parent removes it from `items`, which shows the next photo. */
  onTrash?: (item: AssetListItem) => void;
  /** Open the "add to album" dialog for the current photo. */
  onAddToAlbum?: (item: AssetListItem) => void;
  /** Open the "share link" dialog for the current photo. */
  onShare?: (item: AssetListItem) => void;
  /** Separate the current photo from its RAW (offered in the pair info when the photo has one). */
  onUnpair?: (item: AssetListItem) => void;
  /** Show photos like the current one ("Similar photos"). */
  onSimilar?: (item: AssetListItem) => void;
  /** Favorite or unfavorite the current photo (the parent updates `item.favorite`). Shows the heart when given. */
  onFavorite?: (item: AssetListItem, favorite: boolean) => void;
  /** Position of `items[0]` in the whole list and the size of the whole list, when `items` is only the part that is
   * loaded around the current photo (the counter and the end stops use them). */
  offset?: number;
  total?: number;
  /** Bump after the photo's albums changed so its details (the "In albums" line) are fetched again. */
  refreshKey?: number;
}

export function Lightbox({
  items,
  index,
  onIndexChange,
  onClose,
  loadDetail,
  loadDetails,
  onTrash,
  onAddToAlbum,
  onShare,
  onUnpair,
  onSimilar,
  onFavorite,
  offset = 0,
  total = items.length,
  refreshKey = 0,
}: Props) {
  const item = items[index];
  // Which photo's file chooser is open (it closes by itself when the photo changes).
  const [downloadsFor, setDownloadsFor] = useState<string | null>(null);
  const downloadsOpen = item !== undefined && downloadsFor === item.id;
  const [details, setDetails] = useState<Record<string, AssetDetail>>({});
  const [failed, setFailed] = useState<Record<string, true>>({});
  const requested = useRef(new Set<string>());
  const dialogRef = useRef<HTMLDivElement>(null);
  const [imgLoaded, setImgLoaded] = useState<string | null>(null);

  // The Info panel: swipe up on the photo (phone), the Info button or `i`. It stays open while paging through photos.
  const info = useInfoHistory();
  const [infoById, setInfoById] = useState<Record<string, PhotoDetails>>({});
  const [infoFailed, setInfoFailed] = useState<Record<string, true>>({});
  const infoRequested = useRef(new Set<string>());
  const infoButton = useRef<HTMLButtonElement>(null);
  const currentId = items[index]?.id;
  const [infoRetry, setInfoRetry] = useState(0);
  useEffect(() => {
    if (!info.open || !loadDetails || !currentId || infoRequested.current.has(currentId)) return;
    infoRequested.current.add(currentId);
    const id = currentId;
    loadDetails(id)
      .then((d) => setInfoById((m) => ({ ...m, [id]: d })))
      .catch(() => {
        infoRequested.current.delete(id);
        setInfoFailed((m) => ({ ...m, [id]: true }));
      });
  }, [info.open, loadDetails, currentId, infoRetry]);
  const swipe = useVerticalSwipe({ onUp: info.show, enabled: !!loadDetails && !info.open });
  const closeInfo = info.hide;
  // Closing (or leaving) with the panel open also pops its history entry.
  const { leave } = info;
  const close = useCallback(() => void leave().then(onClose), [leave, onClose]);
  // Closing the panel gives focus back to the button that opens it.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (wasOpen.current && !info.open) infoButton.current?.focus();
    wasOpen.current = info.open;
  }, [info.open]);

  // Fetch fresh presigned URLs for the current photo and its neighbours so ←/→ feels instant.
  useEffect(() => {
    for (const i of [index, index + 1, index - 1]) {
      const id = items[i]?.id;
      if (!id || requested.current.has(id)) continue;
      requested.current.add(id);
      loadDetail(id)
        .then((d) => setDetails((m) => ({ ...m, [id]: d })))
        .catch(() => {
          requested.current.delete(id);
          setFailed((m) => ({ ...m, [id]: true }));
        });
    }
  }, [index, items, loadDetail]);

  // Albums changed: re-read the current photo's details (fresh URLs and album list), keeping the old ones meanwhile.
  const lastRefresh = useRef(refreshKey);
  useEffect(() => {
    if (lastRefresh.current === refreshKey) return;
    lastRefresh.current = refreshKey;
    const id = items[index]?.id;
    if (!id) return;
    loadDetail(id)
      .then((d) => setDetails((m) => ({ ...m, [id]: d })))
      .catch(() => undefined);
    // (The panel's album list is read again too.)
    if (loadDetails && infoRequested.current.has(id)) {
      loadDetails(id)
        .then((d) => setInfoById((m) => ({ ...m, [id]: d })))
        .catch(() => undefined);
    }
  }, [refreshKey, items, index, loadDetail, loadDetails]);

  const go = useCallback(
    (delta: number) => {
      const next = index + delta;
      if (next >= 0 && next < items.length) onIndexChange(next);
    },
    [index, items.length, onIndexChange],
  );

  // Keyboard: ←/→ navigate, Esc closes, Tab stays inside the dialog.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      // Typing in a dialog opened over the lightbox (add to album) is not a lightbox shortcut.
      if ((e.target as HTMLElement | null)?.closest("input, textarea, select")) return;
      if (e.key === "Escape") {
        e.preventDefault();
        // The panel closes first; a second Escape closes the lightbox.
        if (info.open) closeInfo();
        else close();
      } else if (
        e.key === "i" &&
        loadDetails &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        !e.repeat &&
        // Not while a dialog opened over the lightbox (add to album, share) owns the keyboard.
        [null, dialogRef.current].includes((e.target as HTMLElement | null)?.closest('[role="dialog"]') ?? null)
      ) {
        e.preventDefault();
        info.toggle();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        go(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        go(1);
      } else if ((e.key === "Delete" || e.key === "Backspace") && onTrash && !e.repeat) {
        const cur = items[index];
        if (!cur) return;
        e.preventDefault();
        onTrash(cur);
      } else if (e.key === "Tab" && dialogRef.current) {
        const f = dialogRef.current.querySelectorAll<HTMLElement>("button, a[href]");
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
  }, [go, close, onTrash, items, index, info, closeInfo, loadDetails]);

  // Focus management: move focus in, give it back to the tile on close; lock page scroll.
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

  if (!item) return null;
  const detail = details[item.id];
  const src = detail?.preview_url ?? null;
  const fallback = detail?.thumb_url ?? item.thumb_url;
  const bg = placeholderUrl(item.thumbhash);
  const showPending = detail && !src;
  const date = new Date(item.sort_time);
  const paired = detail?.paired ?? item.paired ?? [];
  const favorite = item.favorite === true;
  // The files that can be saved: the photo itself and, for a RAW + JPEG pair, its RAW.
  const files = [
    { key: item.id, href: downloadHref(item.id), label: paired.length > 0 ? primaryFormat(item) : "", size: item.size_bytes },
    ...paired.map((p) => ({ key: p.id, href: downloadHref(item.id, p.id), label: "RAW", size: p.size_bytes })),
  ];
  const position = offset + index;
  const retryInfo = () => {
    setInfoFailed((m) => Object.fromEntries(Object.entries(m).filter(([k]) => k !== item.id)));
    setInfoRetry((n) => n + 1);
  };
  const infoState: DetailsState = infoById[item.id]
    ? { status: "ready", details: infoById[item.id]! }
    : infoFailed[item.id]
      ? { status: "error" }
      : { status: "loading" };

  return (
    <div className="lightbox" role="dialog" aria-modal="true" aria-label={item.original_name} ref={dialogRef}>
      <div className="lightbox-bar">
        <button type="button" className="btn btn-glass" onClick={close}>
          Close
        </button>
        <span className="lightbox-count" aria-live="polite">
          {position + 1} / {total}
        </span>
        <div className="lightbox-right">
          {loadDetails && (
            <button
              type="button"
              ref={infoButton}
              className={`btn btn-glass lightbox-info${info.open ? " on" : ""}`}
              aria-label="Info"
              aria-expanded={info.open}
              aria-controls="details-panel"
              aria-keyshortcuts="i"
              onClick={info.toggle}
            >
              <InfoIcon />
              <span className="lightbox-info-label">Info</span>
            </button>
          )}
          {onSimilar && (
            <button type="button" className="btn btn-glass lightbox-similar" aria-label="Similar photos" onClick={() => onSimilar(item)}>
              <SimilarIcon />
              <span className="lightbox-similar-label">Similar photos</span>
            </button>
          )}
          {onFavorite && (
            <button
              type="button"
              className={`btn btn-glass lightbox-heart${favorite ? " on" : ""}`}
              aria-pressed={favorite}
              aria-label={favorite ? "Remove from favorites" : "Add to favorites"}
              onClick={() => onFavorite(item, !favorite)}
            >
              <HeartIcon filled={favorite} />
            </button>
          )}
          <div className="lightbox-actions">
            {files.length === 1 ? (
              <a className="btn btn-glass" href={files[0]!.href} rel="noopener">
                Download
              </a>
            ) : (
              <button
                type="button"
                className="btn btn-glass"
                aria-expanded={downloadsOpen}
                aria-controls="lightbox-downloads"
                onClick={() => setDownloadsFor(downloadsOpen ? null : item.id)}
              >
                Download
              </button>
            )}
            {onAddToAlbum && (
              <button type="button" className="btn btn-glass" onClick={() => onAddToAlbum(item)}>
                Add to album
              </button>
            )}
            {onShare && (
              <button type="button" className="btn btn-glass" onClick={() => onShare(item)}>
                Share
              </button>
            )}
            {onTrash && (
              <button type="button" className="btn btn-glass" onClick={() => onTrash(item)}>
                Move to trash
              </button>
            )}
          </div>
        </div>
      </div>
      {files.length > 1 && downloadsOpen && (
        <div className="lightbox-downloads" id="lightbox-downloads">
          {files.map((f) => (
            <a key={f.key} className="btn btn-glass" href={f.href} rel="noopener">
              Download {f.label} <span className="muted">{fileSize(f.size)}</span>
            </a>
          ))}
        </div>
      )}
      <div className={`lightbox-body${info.open ? " with-info" : ""}`}>
      <div className="lightbox-main">
      <div className="lightbox-stage" {...swipe} style={bg ? { backgroundImage: `url(${bg})` } : undefined}>
        <button
          type="button"
          className="nav nav-prev"
          aria-label="Previous photo"
          onClick={() => go(-1)}
          disabled={position === 0}
        >
          ‹
        </button>
        {(src ?? fallback) && (
          <img
            key={item.id}
            src={src ?? fallback ?? undefined}
            alt={item.original_name}
            className={src && imgLoaded === item.id ? "loaded" : src ? "loading" : "loaded"}
            onLoad={() => setImgLoaded(item.id)}
          />
        )}
        {failed[item.id] && <p className="lightbox-msg">Could not load this photo.</p>}
        {showPending && !fallback && <p className="lightbox-msg">This photo is still being processed.</p>}
        <button
          type="button"
          className="nav nav-next"
          aria-label="Next photo"
          onClick={() => go(1)}
          disabled={position >= total - 1}
        >
          ›
        </button>
      </div>
      <p className="lightbox-caption">
        <strong>{item.original_name}</strong>
        <span className="muted">
          {item.width && item.height ? `${item.width} × ${item.height} · ` : ""}
          <time dateTime={item.sort_time}>{Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString()}</time>
          {detail?.place && <span className="lightbox-place"> · {placeLine(detail.place)}</span>}
        </span>
      </p>
      {paired.length > 0 && !info.open && (
        <div className="lightbox-pair">
          <strong>{pairLabel(item, paired)}</strong>
          <ul>
            <li>
              {item.original_name} <span className="muted">{fileSize(item.size_bytes)}</span>
            </li>
            {paired.map((p) => (
              <li key={p.id}>
                {p.original_name} <span className="muted">{fileSize(p.size_bytes)}</span>
              </li>
            ))}
          </ul>
          {onUnpair && (
            <button type="button" className="btn btn-glass" onClick={() => onUnpair(item)}>
              Unpair
            </button>
          )}
        </div>
      )}
      {detail?.albums && detail.albums.length > 0 && !info.open && (
        <p className="lightbox-albums">
          <span className="muted">In albums:</span>{" "}
          {detail.albums.map((a, i) => (
            <span key={a.id}>
              {i > 0 && ", "}
              <Link to={`/albums/${a.id}`} onClick={onClose}>
                {a.title}
              </Link>
            </span>
          ))}
        </p>
      )}
      </div>
      {info.open && loadDetails && (
        <DetailsPanel
          panelId="details-panel"
          state={infoState}
          onRetry={retryInfo}
          onClose={closeInfo}
          onLeave={leave}
          onNavigate={onClose}
        />
      )}
      </div>
    </div>
  );
}

/** A circled "i". */
function InfoIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5.5" />
      <circle cx="12" cy="7.8" r="0.6" fill="currentColor" />
    </svg>
  );
}

/** Two overlapping frames. On a phone this is all the button shows (the bar has no room for its words). */
function SimilarIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" aria-hidden="true">
      <rect x="3" y="7" width="13" height="13" rx="2" />
      <path d="M8 3h11a2 2 0 0 1 2 2v11" strokeLinecap="round" />
    </svg>
  );
}
