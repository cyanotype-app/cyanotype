// Turns a list entry into a picture that is ready to draw: ask for a fresh preview address, load the image and decode it, so
// the slideshow can swap it in without a flash. Previews only, never the original.
import type { SlideItem, SlideSource } from "./deck";
import type { Loaded } from "./controller";
import { PreviewCache } from "./previews";

/** Load and decode an image; null when it cannot be loaded (gone, expired, no connection). */
export async function decodeImage(url: string): Promise<HTMLImageElement | null> {
  const img = new Image();
  img.decoding = "async";
  img.src = url;
  try {
    if (typeof img.decode === "function") {
      await img.decode();
    } else {
      await new Promise<void>((resolve, reject) => {
        img.onload = () => resolve();
        img.onerror = () => reject(new Error("image"));
      });
    }
  } catch {
    // decode() can reject for a picture the browser did draw; trust what it loaded.
    if (!(img.complete && img.naturalWidth > 0)) {
      img.removeAttribute("src");
      return null;
    }
  }
  return img;
}

/**
 * The loader a slideshow uses: previews through `cache` (or the source's own way, for a share link), at most the next few
 * addresses asked for together.
 */
export function makeLoader(source: SlideSource, cache: PreviewCache = new PreviewCache(), decode = decodeImage) {
  return async (item: SlideItem, ahead: SlideItem[]): Promise<Loaded | null> => {
    let url: string;
    let caption: string | null;
    if (source.resolve) {
      const r = await source.resolve(item);
      if (!r) return null;
      url = r.url;
      caption = r.caption ?? null;
    } else {
      const info = await cache.get(
        item.id,
        ahead.map((a) => a.id),
      );
      if (!info) return null;
      url = info.preview_url;
      caption = info.caption;
    }
    const img = await decode(url);
    if (!img) {
      // The address may have expired: ask again next time.
      cache.forget(item.id);
      return null;
    }
    return {
      item,
      url,
      caption,
      img,
      width: img.naturalWidth || item.width || null,
      height: img.naturalHeight || item.height || null,
    };
  };
}
