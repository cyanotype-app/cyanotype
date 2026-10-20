// Rotate and crop: the recipe, its geometry and the words for it. Pure functions; the editor (RotateCrop.tsx) and the
// server (crates/cyanotype-core/src/recipe.rs) agree on the same arithmetic, which these tests pin down.

export interface Crop {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** What the editor works on. `crop` is a rectangle in fractions of the frame (after the quarter turns), null for none. */
export interface Recipe {
  /** Clockwise degrees: 0, 90, 180 or 270. */
  rotate: number;
  /** Degrees, positive turns the picture clockwise. */
  straighten: number;
  crop: Crop | null;
}

/** The recipe as the server stores and sends it: parts that are not set are left out. */
export interface ServerRecipe {
  rotate?: number;
  straighten?: number;
  crop?: Crop;
}

export const IDENTITY: Recipe = { rotate: 0, straighten: 0, crop: null };
export const MAX_STRAIGHTEN = 45;
/** The smallest side of a crop, as a fraction of the frame (the server refuses less than 0.02). */
export const MIN_CROP = 0.05;

export function fromServer(r: ServerRecipe | null | undefined): Recipe {
  return { rotate: r?.rotate ?? 0, straighten: r?.straighten ?? 0, crop: r?.crop ? { ...r.crop } : null };
}

const near = (a: number, b: number, eps = 0.0005) => Math.abs(a - b) < eps;

export function isFullCrop(c: Crop): boolean {
  return near(c.x, 0) && near(c.y, 0) && near(c.w, 1) && near(c.h, 1);
}

export function isIdentity(r: Recipe): boolean {
  return r.rotate % 360 === 0 && Math.abs(r.straighten) < 0.05 && (r.crop === null || isFullCrop(r.crop));
}

export function sameRecipe(a: Recipe, b: Recipe): boolean {
  const ca = a.crop && !isFullCrop(a.crop) ? a.crop : null;
  const cb = b.crop && !isFullCrop(b.crop) ? b.crop : null;
  return (
    a.rotate % 360 === b.rotate % 360 &&
    near(a.straighten, b.straighten, 0.05) &&
    (ca === null ? cb === null : cb !== null && near(ca.x, cb.x) && near(ca.y, cb.y) && near(ca.w, cb.w) && near(ca.h, cb.h))
  );
}

/** The body of `POST /assets/edits` for `recipe`: the recipe, or a revert when it changes nothing. */
export function toRequest(r: Recipe): { rotate: number; straighten: number; crop?: Crop } | { revert: true } {
  if (isIdentity(r)) return { revert: true };
  const round = (v: number) => Math.round(v * 10000) / 10000;
  const out: { rotate: number; straighten: number; crop?: Crop } = {
    rotate: ((r.rotate % 360) + 360) % 360,
    straighten: Math.round(r.straighten * 10) / 10,
  };
  if (r.crop && !isFullCrop(r.crop)) out.crop = { x: round(r.crop.x), y: round(r.crop.y), w: round(r.crop.w), h: round(r.crop.h) };
  return out;
}

/** The frame after the quarter turns, for a picture that is `w` x `h` as displayed. */
export function frameSize(r: Recipe, w: number, h: number): [number, number] {
  return r.rotate % 180 === 90 ? [h, w] : [w, h];
}

/** The size of the result: the frame cut to the crop. */
export function outputSize(r: Recipe, w: number, h: number): [number, number] {
  const [fw, fh] = frameSize(r, w, h);
  if (!r.crop) return [fw, fh];
  return [Math.max(1, Math.round(fw * r.crop.w)), Math.max(1, Math.round(fh * r.crop.h))];
}

/** How much the turned picture is magnified so no empty corner shows: `cos a + sin a * max(fw/fh, fh/fw)`. */
export function straightenZoom(degrees: number, fw: number, fh: number): number {
  if (degrees === 0 || fw <= 0 || fh <= 0) return 1;
  const a = (Math.abs(degrees) * Math.PI) / 180;
  return Math.cos(a) + Math.sin(a) * Math.max(fw / fh, fh / fw);
}

/** `r` turned `quarters` quarter turns clockwise (negative: counterclockwise); the crop turns with the picture. */
export function turn(r: Recipe, quarters: number): Recipe {
  let out: Recipe = { ...r, crop: r.crop ? { ...r.crop } : null };
  const n = ((quarters % 4) + 4) % 4;
  for (let i = 0; i < n; i++) {
    const c = out.crop;
    out = {
      rotate: (out.rotate + 90) % 360,
      straighten: out.straighten,
      crop: c ? { x: 1 - c.y - c.h, y: c.x, w: c.h, h: c.w } : null,
    };
  }
  return out;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "move";

export interface DragOptions {
  /** The frame's size in pixels (only its aspect matters): a ratio is a ratio of pixels, not of fractions. */
  fw: number;
  fh: number;
  /** Keep width / height at this ratio (pixels); null for a free crop. */
  ratio: number | null;
  min?: number;
}

/**
 * The crop after dragging `handle` of `start` by `dx`, `dy` (fractions of the frame). The crop stays inside the frame and
 * at least `min` on each side; with a ratio the side that was not dragged follows, anchored at the opposite side.
 */
export function dragCrop(start: Crop, handle: Handle, dx: number, dy: number, o: DragOptions): Crop {
  const min = o.min ?? MIN_CROP;
  if (handle === "move") {
    return { ...start, x: clamp(start.x + dx, 0, 1 - start.w), y: clamp(start.y + dy, 0, 1 - start.h) };
  }
  let l = start.x;
  let t = start.y;
  let r = start.x + start.w;
  let b = start.y + start.h;
  if (handle.includes("w")) l = clamp(l + dx, 0, r - min);
  if (handle.includes("e")) r = clamp(r + dx, l + min, 1);
  if (handle.includes("n")) t = clamp(t + dy, 0, b - min);
  if (handle.includes("s")) b = clamp(b + dy, t + min, 1);
  if (o.ratio === null) return { x: l, y: t, w: r - l, h: b - t };

  // With a ratio: w * fw = ratio * h * fh, so w = k * h.
  const k = (o.ratio * o.fh) / o.fw;
  const horizontal = handle.includes("w") || handle.includes("e");
  let w = r - l;
  let h = b - t;
  // A corner and a left/right edge are led by the width; a top/bottom edge by the height.
  if (horizontal) h = w / k;
  else w = h * k;
  // Where each axis is anchored: the side opposite the handle, or the centre for an edge that only moves the other axis.
  const ax = handle.includes("w") ? "r" : handle.includes("e") ? "l" : "c";
  const ay = handle.includes("n") ? "b" : handle.includes("s") ? "t" : "c";
  const cx = (l + r) / 2;
  const cy = (t + b) / 2;
  const roomX = ax === "r" ? r : ax === "l" ? 1 - l : 2 * Math.min(cx, 1 - cx);
  const roomY = ay === "b" ? b : ay === "t" ? 1 - t : 2 * Math.min(cy, 1 - cy);
  const s = Math.min(1, roomX / w, roomY / h);
  w *= s;
  h *= s;
  return {
    x: ax === "r" ? r - w : ax === "l" ? l : cx - w / 2,
    y: ay === "b" ? b - h : ay === "t" ? t : cy - h / 2,
    w,
    h,
  };
}

/** The largest crop of `ratio` (pixels, width / height) centred in `within` (a crop, or the whole frame when null). */
export function fitRatio(within: Crop | null, ratio: number, fw: number, fh: number): Crop {
  const box = within ?? { x: 0, y: 0, w: 1, h: 1 };
  const k = (ratio * fh) / fw; // w = k * h in fractions
  let w = box.w;
  let h = w / k;
  if (h > box.h) {
    h = box.h;
    w = h * k;
  }
  return { x: box.x + (box.w - w) / 2, y: box.y + (box.h - h) / 2, w, h };
}

export interface RatioChoice {
  id: string;
  label: string;
  /** Width / height in pixels; null is a free crop; "original" follows the frame. */
  ratio: number | "original" | null;
}

export const RATIOS: RatioChoice[] = [
  { id: "free", label: "Free", ratio: null },
  { id: "original", label: "Original", ratio: "original" },
  { id: "1:1", label: "1:1", ratio: 1 },
  { id: "4:3", label: "4:3", ratio: 4 / 3 },
  { id: "3:4", label: "3:4", ratio: 3 / 4 },
  { id: "3:2", label: "3:2", ratio: 3 / 2 },
  { id: "2:3", label: "2:3", ratio: 2 / 3 },
  { id: "16:9", label: "16:9", ratio: 16 / 9 },
  { id: "9:16", label: "9:16", ratio: 9 / 16 },
];

/** The ratio of a choice for a frame of `fw` x `fh`. */
export function ratioOf(choice: RatioChoice, fw: number, fh: number): number | null {
  return choice.ratio === "original" ? fw / fh : choice.ratio;
}

/** A short description of what a recipe does: "Rotated 90° right, straightened 1.3°, cropped to 1500 × 4000". */
export function recipeText(r: Recipe | null | undefined, w?: number | null, h?: number | null): string {
  if (!r || isIdentity(r)) return "Not edited";
  const parts: string[] = [];
  const quarter = ((r.rotate % 360) + 360) % 360;
  if (quarter === 90) parts.push("rotated 90° right");
  else if (quarter === 180) parts.push("rotated 180°");
  else if (quarter === 270) parts.push("rotated 90° left");
  if (Math.abs(r.straighten) >= 0.05) parts.push(`straightened ${Math.abs(r.straighten).toFixed(1)}° ${r.straighten > 0 ? "right" : "left"}`);
  if (r.crop && !isFullCrop(r.crop)) {
    if (w && h) {
      const [ow, oh] = outputSize(r, w, h);
      parts.push(`cropped to ${ow} × ${oh}`);
    } else parts.push("cropped");
  }
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The label of the crop handle, for screen readers. */
export function handleLabel(h: Handle): string {
  switch (h) {
    case "nw":
      return "Crop corner, top left";
    case "ne":
      return "Crop corner, top right";
    case "se":
      return "Crop corner, bottom right";
    case "sw":
      return "Crop corner, bottom left";
    case "n":
      return "Crop edge, top";
    case "s":
      return "Crop edge, bottom";
    case "e":
      return "Crop edge, right";
    case "w":
      return "Crop edge, left";
    default:
      return "Crop area";
  }
}
