import { memo, useRef, useState } from "react";
import type { AssetListItem } from "../api";
import { placeholderUrl } from "../thumbhash";

/** Press and hold this long (touch / pen) to start selecting. */
const LONG_PRESS_MS = 500;
/** Finger travel that turns a press into a scroll. */
const LONG_PRESS_SLOP_PX = 10;

export interface TileProps {
  item: AssetListItem;
  /** Passed back to `onOpen` (the position in whatever list the page shows). */
  index: number;
  onOpen: (index: number) => void;
  /** The thumbnail failed to load (typically an expired presigned URL): ask for a fresh one. */
  onThumbError: (id: string) => void;
  selecting: boolean;
  selected: boolean;
  onSelect?: (id: string, shift: boolean) => void;
  onLongPress?: (id: string) => void;
  /** Shows the heart: on hover (desktop) and always while favorited. Omit where favorites do not apply. */
  onFavorite?: (id: string, favorite: boolean) => void;
  caption?: string | null;
  /** This photo is the cover of a burst (rapid-fire shots of one moment): `more` photos are behind it. The "+N" badge shows
   * or hides them; while it is collapsed a click in selection mode selects the whole burst. */
  burst?: { more: number; expanded: boolean; onToggle: () => void } | null;
  /** This photo is shown because its burst is expanded. */
  inBurst?: boolean;
}

export function HeartIcon({ filled }: { filled: boolean }) {
  return (
    <svg viewBox="0 0 24 24" width="100%" height="100%" aria-hidden="true" focusable="false">
      <path
        d="M12 20.7 4.6 13.4a4.9 4.9 0 0 1 0-6.9 4.8 4.8 0 0 1 6.8 0l.6.6.6-.6a4.8 4.8 0 0 1 6.8 0 4.9 4.9 0 0 1 0 6.9z"
        fill={filled ? "currentColor" : "none"}
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** One photo: a button that opens it (or toggles it while selecting), with the heart and RAW badge on top. */
export const Tile = memo(function Tile({
  item,
  index,
  onOpen,
  onThumbError,
  selecting,
  selected,
  onSelect,
  onLongPress,
  onFavorite,
  caption = null,
  burst = null,
  inBurst = false,
}: TileProps) {
  const [loaded, setLoaded] = useState(false);
  const [retried, setRetried] = useState(false);
  const press = useRef<{ timer: ReturnType<typeof setTimeout>; x: number; y: number } | null>(null);
  const longPressed = useRef(false);
  const bg = placeholderUrl(item.thumbhash);
  const favorite = item.favorite === true;

  const cancelPress = () => {
    if (press.current) clearTimeout(press.current.timer);
    press.current = null;
  };
  const canLongPress = !selecting && onLongPress !== undefined;

  return (
    <div className={`tile-cell${favorite ? " is-favorite" : ""}${inBurst ? " in-burst" : ""}${burst ? " burst-cover" : ""}`}>
      <button
        type="button"
        className={`tile${selected ? " selected" : ""}`}
        style={bg ? { backgroundImage: `url(${bg})` } : undefined}
        onClick={(e) => {
          if (longPressed.current) {
            // The click that ends a long-press must not also open the photo.
            longPressed.current = false;
            return;
          }
          if (selecting) onSelect?.(item.id, e.shiftKey);
          else onOpen(index);
        }}
        onPointerDown={(e) => {
          longPressed.current = false;
          if (!canLongPress || e.pointerType === "mouse") return;
          cancelPress();
          press.current = {
            x: e.clientX,
            y: e.clientY,
            timer: setTimeout(() => {
              press.current = null;
              longPressed.current = true;
              onLongPress(item.id);
            }, LONG_PRESS_MS),
          };
        }}
        onPointerMove={(e) => {
          const p = press.current;
          if (p && Math.hypot(e.clientX - p.x, e.clientY - p.y) > LONG_PRESS_SLOP_PX) cancelPress();
        }}
        onPointerUp={cancelPress}
        onPointerCancel={cancelPress}
        onPointerLeave={cancelPress}
        onContextMenu={(e) => {
          // Touch long-press would open the browser's image menu instead of selecting.
          if (canLongPress || selecting) e.preventDefault();
        }}
        aria-label={
          selecting
            ? burst && !burst.expanded
              ? `Select ${item.original_name} and ${burst.more} more from its burst`
              : `Select ${item.original_name}`
            : `Open ${item.original_name}`
        }
        aria-pressed={selecting ? selected : undefined}
      >
        {item.thumb_url ? (
          <img
            src={item.thumb_url}
            alt={item.original_name}
            loading="lazy"
            decoding="async"
            draggable={false}
            className={loaded ? "loaded" : undefined}
            onLoad={() => setLoaded(true)}
            onError={() => {
              if (!retried) {
                setRetried(true);
                onThumbError(item.id);
              }
            }}
          />
        ) : (
          <span className="tile-pending">Processing…</span>
        )}
        {item.paired && item.paired.length > 0 && (
          <span className="tile-badge" title="RAW + JPEG: the RAW file is kept with this photo">
            RAW
          </span>
        )}
        {item.archived === true && (
          <span className="tile-badge tile-badge-archived" title="Archived: not in the timeline or the map">
            Archived
          </span>
        )}
        {selecting && (
          <span className="tile-check" aria-hidden="true">
            {selected ? "✓" : ""}
          </span>
        )}
        {caption && <span className="tile-caption">{caption}</span>}
      </button>
      {burst && (
        <button
          type="button"
          className={`tile-burst${burst.expanded ? " open" : ""}`}
          aria-expanded={burst.expanded}
          aria-label={
            burst.expanded
              ? `Hide the other ${burst.more} photos of this burst`
              : `Show ${burst.more} more photos from this burst`
          }
          onClick={burst.onToggle}
        >
          {burst.expanded ? "−" : `+${burst.more}`}
        </button>
      )}
      {onFavorite &&
        (selecting ? (
          favorite && (
            <span className="tile-heart on" aria-hidden="true">
              <HeartIcon filled />
            </span>
          )
        ) : (
          <button
            type="button"
            className={`tile-heart${favorite ? " on" : ""}`}
            aria-label={favorite ? `Remove ${item.original_name} from favorites` : `Add ${item.original_name} to favorites`}
            aria-pressed={favorite}
            onClick={() => onFavorite(item.id, !favorite)}
          >
            <HeartIcon filled={favorite} />
          </button>
        ))}
    </div>
  );
});
