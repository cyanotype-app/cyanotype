// The viewer's zoom, as pure functions (unit tested; the gesture handling in usePhotoZoom only moves numbers through them).
//
// A view is how the photo is shown: `scale` is relative to the fitted photo (1 = fit, the smallest), and `x`, `y` is where
// the photo's centre sits relative to the centre of the stage, in screen pixels. A point `q` of the fitted photo (measured
// from its centre) is then at `scale * q + (x, y)`. Points the gestures work with (cursor, pinch midpoint) are also
// measured from the centre of the stage.

export interface View {
  scale: number;
  x: number;
  y: number;
}

export interface Size {
  w: number;
  h: number;
}

export interface Point {
  x: number;
  y: number;
}

export const FIT: View = { scale: 1, x: 0, y: 0 };

/** The largest zoom, as a multiple of the fitted photo, whatever the picture: past this nothing more can be seen. */
export const MAX_SCALE = 8;
/** At least this much zoom is always allowed, even for a small picture (so a double tap does something). */
export const MIN_MAX_SCALE = 2;
/** The most a picture's own pixels are enlarged: 4 means one pixel of the picture covers 4 x 4 device pixels. */
export const MAX_PIXEL_ENLARGEMENT = 4;
/** A button or key press multiplies (or divides) the zoom by this. */
export const STEP = 1.5;
/** Below this a zoom is read as "fit" (a pinch that ends almost back at fit snaps to it). */
export const SNAP_SCALE = 1.05;

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

/**
 * How far in the photo may be zoomed: 4 device pixels per pixel of the best picture there is (`naturalLong`, its long edge),
 * never less than 2x fit and never more than 8x fit. `fitLong` is the fitted photo's long edge in CSS pixels, `dpr` the
 * screen's device pixel ratio. (4 pixel-peeps at 400 %, which is as far as a picture shows anything new; 8x fit stops a
 * 50 MP photo from zooming into mush on a phone, where the thumb cannot place such a gesture anyway.)
 */
export function maxScale(naturalLong: number, fitLong: number, dpr: number): number {
  if (!(naturalLong > 0) || !(fitLong > 0)) return MIN_MAX_SCALE;
  return clamp((MAX_PIXEL_ENLARGEMENT * naturalLong) / (fitLong * Math.max(dpr, 1)), MIN_MAX_SCALE, MAX_SCALE);
}

/** The zoom a double tap goes to: one picture pixel per device pixel, or 2x fit when that is less (and no more than `max`). */
export function doubleTapScale(naturalLong: number, fitLong: number, dpr: number, max: number): number {
  const actual = naturalLong > 0 && fitLong > 0 ? naturalLong / (fitLong * Math.max(dpr, 1)) : 0;
  return clamp(Math.max(2, actual), 1, max);
}

/** Keep the photo on screen: it can be dragged until its edge meets the stage's, and no further (a photo smaller than the stage on an axis stays centred there). */
export function clampView(v: View, stage: Size, photo: Size, max = MAX_SCALE): View {
  const scale = clamp(v.scale, 1, Math.max(1, max));
  const room = (stageSide: number, photoSide: number) => Math.max(0, (photoSide * scale - stageSide) / 2);
  const rx = room(stage.w, photo.w);
  const ry = room(stage.h, photo.h);
  // (`+ 0` turns a -0 into 0.)
  return { scale, x: clamp(v.x, -rx, rx) + 0, y: clamp(v.y, -ry, ry) + 0 };
}

/** Zoom to `scale` keeping the photo point under `at` where it is (the cursor, the pinch midpoint, a double tap). */
export function zoomAt(v: View, scale: number, at: Point, stage: Size, photo: Size, max = MAX_SCALE): View {
  const next = clamp(scale, 1, Math.max(1, max));
  const k = next / v.scale;
  return clampView({ scale: next, x: at.x - (at.x - v.x) * k, y: at.y - (at.y - v.y) * k }, stage, photo, max);
}

/** A pinch: the two fingers' distance scales the photo and the midpoint carries it, from the view when the pinch began. */
export function pinchView(start: View, from: [Point, Point], to: [Point, Point], stage: Size, photo: Size, max = MAX_SCALE): View {
  const d0 = Math.hypot(from[0].x - from[1].x, from[0].y - from[1].y);
  const d1 = Math.hypot(to[0].x - to[1].x, to[0].y - to[1].y);
  const m0 = { x: (from[0].x + from[1].x) / 2, y: (from[0].y + from[1].y) / 2 };
  const m1 = { x: (to[0].x + to[1].x) / 2, y: (to[0].y + to[1].y) / 2 };
  const ratio = d0 > 0 ? d1 / d0 : 1;
  const scale = clamp(start.scale * ratio, 1, Math.max(1, max));
  const k = scale / start.scale;
  // The photo point that was under the first midpoint ends under the second.
  return clampView({ scale, x: m1.x - (m0.x - start.x) * k, y: m1.y - (m0.y - start.y) * k }, stage, photo, max);
}

/** The next zoom for a button or key press (`dir` 1 in, -1 out), snapping to fit near the bottom and to `max` near the top. */
export function stepScale(scale: number, dir: 1 | -1, max: number): number {
  const next = dir > 0 ? scale * STEP : scale / STEP;
  if (next < SNAP_SCALE) return 1;
  return clamp(next, 1, max);
}

/** How much a wheel event with ctrl held (a trackpad pinch, or ctrl + mouse wheel) zooms: smooth for the trackpad's small deltas, a firm step for a wheel notch. */
export function wheelFactor(deltaY: number, deltaMode: number): number {
  const px = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 100 : deltaY;
  return Math.exp(-clamp(px, -60, 60) * 0.01);
}

/** "150 %": the zoom as fit = 100 %. */
export function percentLabel(scale: number): string {
  return `${Math.round(scale * 100)} %`;
}

/**
 * Whether the zoom is past what the preview can show sharply: the photo drawn at `scale` is wider (in device pixels) than
 * the preview's own pixels. At fit nothing is asked for, whatever the screen.
 */
export function pastPreview(scale: number, fitLong: number, previewLong: number, dpr: number): boolean {
  if (scale <= 1.001 || !(previewLong > 0)) return false;
  return scale * fitLong * Math.max(dpr, 1) > previewLong;
}

/** What browsers show in an <img>: the types whose original file can stand in for the preview. */
const DISPLAYABLE = ["image/jpeg", "image/png", "image/webp"];
/** Pictures beyond this many pixels are not loaded in the viewer (a phone decodes such an image slowly or not at all). */
export const MAX_ORIGINAL_PIXELS = 100_000_000;

export interface Rendition {
  contentType: string;
  /** The photo has an edit (rotate/crop): the original is not what is shown. */
  edited: boolean;
  /** The worker is still re-rendering the edited picture. */
  rendering: boolean;
  originalUrl: string | null | undefined;
  /** The original file's size in pixels, when known. */
  width: number | null | undefined;
  height: number | null | undefined;
  /** The long edge of the preview on show. */
  previewLong: number;
}

/**
 * The URL of a sharper picture than the preview, or null when there is none to show: the original file of an unedited
 * JPEG, PNG or WebP that has more pixels than the preview. HEIC, RAW and the rest (browsers cannot show them), edited
 * photos (the original is not the edited picture; the edited JPEG is made on demand, for download) and pictures that are
 * not larger than the preview have none: the viewer then scales the preview.
 */
export function sharperUrl(r: Rendition): string | null {
  if (!r.originalUrl || r.edited || r.rendering || !(r.previewLong > 0)) return null;
  if (!DISPLAYABLE.includes(r.contentType.toLowerCase().split(";")[0]!.trim())) return null;
  const w = r.width ?? 0;
  const h = r.height ?? 0;
  if (!(w > 0) || !(h > 0) || w * h > MAX_ORIGINAL_PIXELS) return null;
  return Math.max(w, h) > r.previewLong * 1.1 ? r.originalUrl : null;
}
