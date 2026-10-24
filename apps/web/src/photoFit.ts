import type { CSSProperties } from "react";

/** How much larger than its own pixels a photo may be drawn when fitted to the viewer (sharper beyond that is zoom's job). */
export const MAX_FIT_UPSCALE = 2;

/**
 * The custom properties the viewer's CSS fits a photo with, from the size the picture has once loaded: `--ar` (width / height)
 * and `--cap` (the widest it is drawn at rest). Without a size the photo is drawn as it is, bounded by the area.
 */
export function fitStyle(size: { w: number; h: number } | null | undefined): CSSProperties | undefined {
  if (!size || size.w <= 0 || size.h <= 0) return undefined;
  return { "--ar": String(size.w / size.h), "--cap": `${Math.round(size.w * MAX_FIT_UPSCALE)}px` } as CSSProperties;
}
