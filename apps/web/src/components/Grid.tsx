import { memo, useRef, useState } from "react";
import type { AssetListItem } from "../api";
import { placeholderUrl } from "../thumbhash";

/** Press and hold this long (touch / pen) to start selecting. */
const LONG_PRESS_MS = 500;
/** Finger travel that turns a press into a scroll. */
const LONG_PRESS_SLOP_PX = 10;

interface Props<T extends AssetListItem> {
  items: T[];
  onOpen: (index: number) => void;
  /** The thumbnail failed to load (typically an expired presigned URL): ask for a fresh one. */
  onThumbError: (id: string) => void;
  /** Selection mode: tiles toggle instead of opening. */
  selecting?: boolean;
  selected?: ReadonlySet<string>;
  /** A tile was clicked / tapped in selection mode; `shift` extends the selection from the last clicked tile. */
  onSelect?: (id: string, shift: boolean) => void;
  /** A tile was long-pressed outside selection mode: the page should enter selection mode with it selected. */
  onLongPress?: (id: string) => void;
  /** Small text over the bottom of a tile (the trash page shows when it will be deleted). */
  caption?: (item: T) => string | null;
}

export function Grid<T extends AssetListItem>({
  items,
  onOpen,
  onThumbError,
  selecting = false,
  selected,
  onSelect,
  onLongPress,
  caption,
}: Props<T>) {
  return (
    <ul className={`grid${selecting ? " selecting" : ""}`} aria-label="Photos">
      {items.map((it, i) => (
        <li key={it.id}>
          <Tile
            item={it}
            index={i}
            onOpen={onOpen}
            onThumbError={onThumbError}
            selecting={selecting}
            selected={selected?.has(it.id) ?? false}
            onSelect={onSelect}
            onLongPress={onLongPress}
            caption={caption?.(it) ?? null}
          />
        </li>
      ))}
    </ul>
  );
}

const Tile = memo(function Tile({
  item,
  index,
  onOpen,
  onThumbError,
  selecting,
  selected,
  onSelect,
  onLongPress,
  caption,
}: {
  item: AssetListItem;
  index: number;
  onOpen: (index: number) => void;
  onThumbError: (id: string) => void;
  selecting: boolean;
  selected: boolean;
  onSelect?: (id: string, shift: boolean) => void;
  onLongPress?: (id: string) => void;
  caption: string | null;
}) {
  const [loaded, setLoaded] = useState(false);
  const [retried, setRetried] = useState(false);
  const press = useRef<{ timer: ReturnType<typeof setTimeout>; x: number; y: number } | null>(null);
  const longPressed = useRef(false);
  const bg = placeholderUrl(item.thumbhash);

  const cancelPress = () => {
    if (press.current) clearTimeout(press.current.timer);
    press.current = null;
  };
  const canLongPress = !selecting && onLongPress !== undefined;

  return (
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
      aria-label={selecting ? `Select ${item.original_name}` : `Open ${item.original_name}`}
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
      {selecting && (
        <span className="tile-check" aria-hidden="true">
          {selected ? "✓" : ""}
        </span>
      )}
      {caption && <span className="tile-caption">{caption}</span>}
    </button>
  );
});
