import { Check, Play } from "lucide-react";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import type { AssetListItem } from "../api";
import { placeholderUrl } from "../thumbhash";
import { stackLabel, tileIds } from "../stacks";
import { cn } from "../ui/cn";

/** Press and hold this long (touch / pen) to start selecting. */
const LONG_PRESS_MS = 420;
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
  /** After a long press, the finger moved over another tile (touch): the page extends the selection to it. */
  onDragOver?: (id: string) => void;
  /** The finger that long-pressed was lifted (or the system took the touch). */
  onDragEnd?: () => void;
  /** Shows the heart: on hover (desktop) and always while favorited. Omit where favorites do not apply. */
  onFavorite?: (id: string, favorite: boolean) => void;
  caption?: string | null;
  /** This photo is the cover of a burst (rapid-fire shots of one moment): `more` photos are behind it. The "+N" badge shows
   * or hides them; while it is collapsed a click in selection mode selects the whole burst. */
  burst?: { more: number; expanded: boolean; onToggle: () => void } | null;
  /** This photo is shown because its burst is expanded. */
  inBurst?: boolean;
}

/** Two sheets, one behind the other: the stack badge. */
export function StackIcon() {
  return (
    <svg viewBox="0 0 24 24" width="100%" height="100%" aria-hidden="true" focusable="false">
      <rect x="8" y="3" width="13" height="13" rx="2" fill="none" stroke="currentColor" strokeWidth="2" />
      <path d="M5 8v10a2 2 0 0 0 2 2h10" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
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

const badge = "pointer-events-none rounded-full bg-black/55 px-1.5 text-[0.625rem] leading-[1.35] font-semibold tracking-wide text-white backdrop-blur-sm";

/**
 * One photo: a button that opens it (or toggles it while selecting), with the heart and the badges on top. It fills whatever
 * cell it is put in; the blurred ThumbHash shows at once and the picture fades in over it.
 */
export const Tile = memo(function Tile({
  item,
  index,
  onOpen,
  onThumbError,
  selecting,
  selected,
  onSelect,
  onLongPress,
  onDragOver,
  onDragEnd,
  onFavorite,
  caption = null,
  burst = null,
  inBurst = false,
}: TileProps) {
  const [loaded, setLoaded] = useState(false);
  const [retried, setRetried] = useState(false);
  const press = useRef<{ timer: ReturnType<typeof setTimeout>; x: number; y: number } | null>(null);
  const longPressed = useRef(false);
  const stopDrag = useRef<(() => void) | null>(null);
  const bg = placeholderUrl(item.thumbhash);
  const favorite = item.favorite === true;
  // A stack in the timeline (two or more photos behind this tile): the badge, and what selecting the tile means.
  const stackSize = item.stack && item.stack.size > 1 ? item.stack.size : 0;
  const behind = tileIds(item).length - 1;
  const video = item.content_type.startsWith("video/");

  const cancelPress = () => {
    if (press.current) clearTimeout(press.current.timer);
    press.current = null;
  };
  useEffect(
    () => () => {
      if (press.current) clearTimeout(press.current.timer);
      stopDrag.current?.();
    },
    [],
  );
  const canLongPress = onLongPress !== undefined;

  // After the long press, a finger that keeps moving selects the tiles it crosses; the page does not scroll meanwhile.
  const followFinger = useCallback(() => {
    if (!onDragOver) return;
    let last = item.id;
    const move = (e: TouchEvent) => {
      const t = e.touches[0];
      if (!t) return;
      if (e.cancelable) e.preventDefault();
      const id = document.elementFromPoint?.(t.clientX, t.clientY)?.closest<HTMLElement>("[data-tile-id]")?.dataset.tileId;
      if (id && id !== last) {
        last = id;
        onDragOver(id);
      }
    };
    const end = () => {
      stopDrag.current?.();
      onDragEnd?.();
    };
    document.addEventListener("touchmove", move, { passive: false });
    document.addEventListener("touchend", end);
    document.addEventListener("touchcancel", end);
    stopDrag.current = () => {
      document.removeEventListener("touchmove", move);
      document.removeEventListener("touchend", end);
      document.removeEventListener("touchcancel", end);
      stopDrag.current = null;
    };
  }, [item.id, onDragOver, onDragEnd]);

  // A picture that was already in the cache is there before React hears about it.
  const imgRef = useCallback((el: HTMLImageElement | null) => {
    if (el?.complete && el.naturalWidth > 0) setLoaded(true);
  }, []);

  const showCheck = selecting;
  return (
    <div
      data-tile=""
      data-tile-id={item.id}
      data-selected={selected ? "" : undefined}
      data-in-burst={inBurst ? "" : undefined}
      data-burst-cover={burst ? "" : undefined}
      className={cn(
        "group relative size-full overflow-hidden bg-surface",
        inBurst && "ring-2 ring-accent ring-inset",
        burst && !burst.expanded && "shadow-[3px_3px_0_-1px_var(--surface-2),6px_6px_0_-2px_var(--border)]",
      )}
    >
      <button
        type="button"
        className={cn(
          "absolute inset-0 block size-full cursor-pointer overflow-hidden bg-cover bg-center p-0 outline-offset-[-2px] select-none [-webkit-touch-callout:none]",
          "transition-[transform,border-radius] duration-200 ease-soft",
          selected && "scale-[0.86] rounded-[0.65rem]",
        )}
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
              navigator.vibrate?.(8);
              onLongPress(item.id);
              if (e.pointerType === "touch") followFinger();
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
              : behind > 0
                ? `Select ${item.original_name} and ${behind} more from its stack`
                : `Select ${item.original_name}`
            : stackSize
              ? `Open ${item.original_name}, ${stackLabel(stackSize).toLowerCase()}`
              : `Open ${item.original_name}`
        }
        aria-pressed={selecting ? selected : undefined}
      >
        {item.thumb_url ? (
          <img
            ref={imgRef}
            src={item.thumb_url}
            alt={item.original_name}
            data-asset-id={item.id}
            loading="lazy"
            decoding="async"
            draggable={false}
            className={cn("absolute inset-0 size-full object-cover opacity-0 transition-opacity duration-300 ease-soft", loaded && "opacity-100")}
            onLoad={() => setLoaded(true)}
            onError={() => {
              if (!retried) {
                setRetried(true);
                onThumbError(item.id);
              }
            }}
          />
        ) : (
          <span data-asset-id={item.id} className="absolute inset-0 grid grid-cols-1 place-items-center text-xs text-muted">
            <span className="rounded-full bg-card/80 px-2 py-0.5">Processing…</span>
          </span>
        )}
        {selecting && !selected && <span className="absolute inset-0 bg-black/10" aria-hidden="true" />}
        <span className="pointer-events-none absolute top-1.5 right-1.5 flex items-center gap-1">
          {video && (
            <span className={cn(badge, "inline-flex items-center py-0.5")} title="Video">
              <Play className="size-2.5 fill-current" aria-hidden="true" />
              <span className="sr-only">Video</span>
            </span>
          )}
          {item.paired && item.paired.length > 0 && (
            <span className={badge} title="RAW + JPEG: the RAW file is kept with this photo">
              RAW
            </span>
          )}
          {item.archived === true && (
            <span className={badge} title="Archived: not in the timeline or the map">
              Archived
            </span>
          )}
        </span>
        {stackSize > 0 && (
          <span className={cn(badge, "absolute right-1.5 bottom-1.5 inline-flex items-center gap-1 py-0.5 pl-1 text-[0.6875rem]")} title={stackLabel(stackSize)}>
            <span className="inline-grid size-3">
              <StackIcon />
            </span>
            <span aria-hidden="true">{stackSize}</span>
          </span>
        )}
        {showCheck && (
          <span
            className={cn(
              "absolute top-1.5 left-1.5 grid size-6 grid-cols-1 place-items-center rounded-full transition-[background-color,box-shadow,transform] duration-150 ease-soft",
              selected
                ? "scale-100 bg-accent text-on-accent shadow-[0_0_0_1.5px_white]"
                : "bg-black/15 shadow-[inset_0_0_0_1.5px_rgb(255_255_255/0.95)]",
            )}
            aria-hidden="true"
          >
            {selected && <Check className="size-4" strokeWidth={3} />}
          </span>
        )}
        {caption && (
          <span className="pointer-events-none absolute inset-x-0 bottom-0 truncate bg-black/55 px-2 py-1 text-xs text-white">{caption}</span>
        )}
      </button>
      {burst && (
        <button
          type="button"
          className={cn(
            "absolute right-1.5 bottom-1.5 min-h-9 min-w-9 rounded-full border border-white/60 px-2 text-xs leading-none font-bold text-white",
            burst.expanded ? "bg-accent" : "bg-black/70 hover:bg-accent",
          )}
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
            <span
              data-heart="on"
              className="pointer-events-none absolute bottom-1.5 left-1.5 grid size-7 grid-cols-1 place-items-center rounded-full bg-black/45 p-1.5 text-[#ff6b81]"
              aria-hidden="true"
            >
              <span className="block size-full">
                <HeartIcon filled />
              </span>
            </span>
          )
        ) : (
          <button
            type="button"
            data-heart={favorite ? "on" : "off"}
            className={cn(
              "absolute bottom-1.5 left-1.5 grid size-8 cursor-pointer grid-cols-1 place-items-center rounded-full bg-black/45 p-1.5 text-white transition-opacity duration-150",
              "focus-visible:opacity-100 group-hover:opacity-100 [@media(hover:none)]:pointer-events-none",
              favorite ? "text-[#ff6b81] opacity-100" : "opacity-0",
            )}
            aria-label={favorite ? `Remove ${item.original_name} from favorites` : `Add ${item.original_name} to favorites`}
            aria-pressed={favorite}
            onClick={() => onFavorite(item.id, !favorite)}
          >
            <span className="block size-full">
              <HeartIcon filled={favorite} />
            </span>
          </button>
        ))}
    </div>
  );
});
