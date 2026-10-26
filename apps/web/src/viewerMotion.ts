// Geometry of the viewer's open and close animation: the photo grows from its tile in the grid and shrinks back into it.

/** A transform of the viewer's photo box (which is centred in its own box) plus a clip, in that box's own pixels. */
export interface Lift {
  x: number;
  y: number;
  scale: number;
  clip: string;
}

export const LIFT_REST: Lift = { x: 0, y: 0, scale: 1, clip: "inset(0px 0px 0px 0px round 0px)" };

/**
 * Where the photo box has to be so that the photo, drawn as large as it fits the box, covers `tile` exactly as the tile's own
 * picture does (cropped to fill it). `box` is the photo box on screen; `ratio` the photo's width / height.
 */
export function liftOnto(tile: { left: number; top: number; width: number; height: number }, box: { left: number; top: number; width: number; height: number }, ratio: number): Lift {
  // The photo as it sits in the box: contained.
  const pw = Math.min(box.width, box.height * ratio);
  const ph = pw / ratio;
  // Scaled so that it covers the tile, then moved so that the centres meet.
  const scale = Math.max(tile.width / pw, tile.height / ph);
  const x = tile.left + tile.width / 2 - (box.left + box.width / 2);
  const y = tile.top + tile.height / 2 - (box.top + box.height / 2);
  // The clip is the tile, seen from inside the (unscaled) box.
  const halfW = tile.width / scale / 2;
  const halfH = tile.height / scale / 2;
  const left = Math.max(0, box.width / 2 - halfW);
  const top = Math.max(0, box.height / 2 - halfH);
  const radius = 6 / scale;
  return { x, y, scale, clip: `inset(${top}px ${left}px ${top}px ${left}px round ${radius}px)` };
}

/** Whether a tile is on screen (a close that shrinks into a tile scrolled out of view fades instead). */
export function onScreen(r: { left: number; top: number; right: number; bottom: number }) {
  return r.bottom > 0 && r.top < window.innerHeight && r.right > 0 && r.left < window.innerWidth;
}

export const OPEN_MS = 0.34;
export const CLOSE_MS = 0.26;
export const EASE = [0.2, 0.8, 0.2, 1] as const;
