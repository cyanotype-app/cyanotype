// A small bridge between the photo grid and the viewer: where a photo's tile is on screen, so the viewer can open from it
// (and close back into it). Every grid tile's picture carries `data-asset-id`; the viewer asks here instead of searching.

const escape = (id: string) => (typeof CSS !== "undefined" && CSS.escape ? CSS.escape(id) : id.replace(/["\\]/g, "\\$&"));

/** The element showing the photo's thumbnail in the grid (its picture, or its placeholder while it has none), if on the page. */
export function tileElement(id: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-tile] [data-asset-id="${escape(id)}"]`);
}

/**
 * Where the photo's tile is, in viewport coordinates; null when the tile is not on the page (its row is far from the screen)
 * or has no size. The rect may lie partly outside the viewport (a tile under the header or the tab bar): the viewer decides
 * whether that is worth animating from.
 */
export function tileRect(id: string): DOMRect | null {
  const el = tileElement(id);
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return r.width > 0 && r.height > 0 ? r : null;
}
