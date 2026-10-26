/**
 * Where a photo's tile is on the page right now (viewport pixels), or null when it is not drawn (scrolled far away, not in
 * a grid at all). The viewer grows its photo from this rectangle and shrinks back into it; tiles carry `data-asset-id`.
 */
export function tileRect(id: string): DOMRect | null {
  if (typeof document === "undefined") return null;
  const el = document.querySelector<HTMLElement>(`[data-asset-id="${id.replace(/["\\]/g, "\\$&")}"]`);
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return r.width > 0 && r.height > 0 ? r : null;
}
