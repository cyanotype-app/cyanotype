import { memo, useState } from "react";
import type { AssetListItem } from "../api";
import { placeholderUrl } from "../thumbhash";

interface Props {
  items: AssetListItem[];
  onOpen: (index: number) => void;
  /** The thumbnail failed to load (typically an expired presigned URL): ask for a fresh one. */
  onThumbError: (id: string) => void;
}

export function Grid({ items, onOpen, onThumbError }: Props) {
  return (
    <ul className="grid" aria-label="Photos">
      {items.map((it, i) => (
        <li key={it.id}>
          <Tile item={it} index={i} onOpen={onOpen} onThumbError={onThumbError} />
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
}: {
  item: AssetListItem;
  index: number;
  onOpen: (index: number) => void;
  onThumbError: (id: string) => void;
}) {
  const [loaded, setLoaded] = useState(false);
  const [retried, setRetried] = useState(false);
  const bg = placeholderUrl(item.thumbhash);
  return (
    <button
      type="button"
      className="tile"
      style={bg ? { backgroundImage: `url(${bg})` } : undefined}
      onClick={() => onOpen(index)}
      aria-label={`Open ${item.original_name}`}
    >
      {item.thumb_url ? (
        <img
          src={item.thumb_url}
          alt={item.original_name}
          loading="lazy"
          decoding="async"
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
    </button>
  );
});
