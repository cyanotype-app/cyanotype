import type { Album } from "../api";
import { placeholderUrl } from "../thumbhash";

/** The album's cover thumbnail over its ThumbHash placeholder, or an empty frame. */
export function AlbumCover({ cover }: { cover: Album["cover"] }) {
  const bg = placeholderUrl(cover?.thumbhash ?? null);
  return (
    <div className="album-cover" style={bg ? { backgroundImage: `url(${bg})` } : undefined}>
      {cover?.thumb_url ? <img src={cover.thumb_url} alt="" loading="lazy" decoding="async" draggable={false} /> : null}
    </div>
  );
}
