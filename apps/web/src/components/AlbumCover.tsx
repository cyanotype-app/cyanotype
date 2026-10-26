import { Images } from "lucide-react";
import type { Album } from "../api";
import { placeholderUrl } from "../thumbhash";
import { cn } from "../ui/cn";

/**
 * The album's cover thumbnail over its ThumbHash placeholder, or an empty frame with a quiet icon. Fills a square: give it
 * the size and the corners (`className`).
 */
export function AlbumCover({ cover, className }: { cover: Album["cover"]; className?: string }) {
  const bg = placeholderUrl(cover?.thumbhash ?? null);
  return (
    <div
      className={cn("relative aspect-square overflow-hidden rounded-card bg-surface bg-cover", className)}
      style={bg ? { backgroundImage: `url(${bg})` } : undefined}
    >
      {cover?.thumb_url ? (
        <img src={cover.thumb_url} alt="" loading="lazy" decoding="async" draggable={false} className="size-full object-cover" />
      ) : (
        <span className="absolute inset-0 flex items-center justify-center text-muted/60" aria-hidden="true">
          <Images className="size-1/3" strokeWidth={1.5} />
        </span>
      )}
    </div>
  );
}
