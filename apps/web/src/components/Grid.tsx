import type { AssetListItem } from "../api";
import { Tile, type TileProps } from "./Tile";

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
  /** Heart on the tiles (hover, and always while favorited). */
  onFavorite?: (id: string, favorite: boolean) => void;
  /** Small text over the bottom of a tile (the trash page shows when it will be deleted). */
  caption?: (item: T) => string | null;
  /** Bursts (search): which tiles cover a burst (and whether it is open), and which are shown because one is. */
  burst?: (item: T) => TileProps["burst"];
  inBurst?: (item: T) => boolean;
}

/** Plain (not virtualized) grid of tiles, for the short lists of albums, the trash and the photo picker. */
export function Grid<T extends AssetListItem>({
  items,
  onOpen,
  onThumbError,
  selecting = false,
  selected,
  onSelect,
  onLongPress,
  onFavorite,
  caption,
  burst,
  inBurst,
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
            onFavorite={onFavorite}
            caption={caption?.(it) ?? null}
            burst={burst?.(it) ?? null}
            inBurst={inBurst?.(it) ?? false}
          />
        </li>
      ))}
    </ul>
  );
}
