import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from "react";
import {
  FIT,
  SNAP_SCALE,
  clampView,
  doubleTapScale,
  maxScale,
  pastPreview,
  percentLabel,
  pinchView,
  stepScale,
  wheelFactor,
  zoomAt,
  type Point,
  type Size,
  type View,
} from "./zoomMath";

/** A press and release within this long, and this far apart, is a tap; two taps this close are a double tap. */
const TAP_MS = 350;
const TAP_SLOP_PX = 10;
const DOUBLE_TAP_SLOP_PX = 40;

export interface ZoomOptions {
  /** The gesture surface (the photo's area). */
  stage: RefObject<HTMLElement | null>;
  /** The layer that gets the transform (the photo and whatever is drawn on it). */
  layer: RefObject<HTMLElement | null>;
  /** The photo, laid out at fit (its unscaled box is what zoom works from). */
  photo: RefObject<HTMLImageElement | null>;
  /** Shows "150 %" (updated directly: a pinch does not re-render the viewer). */
  level: RefObject<HTMLElement | null>;
  /** A live region that says the zoom once a gesture has settled. */
  announce: RefObject<HTMLElement | null>;
  /** The zoom starts again from fit when this changes (the photo on show). */
  resetKey: string | undefined;
  /** Whether there is a photo on stage to listen on. */
  active: boolean;
  /** The long edge of the preview on show (0 until known), and of the best picture there is to zoom into. */
  previewLong: number;
  naturalLong: number;
}

/**
 * Zoom and pan for the viewer's photo, without touching the page: pinch (two pointers), ctrl + wheel (trackpad pinch), a double
 * tap or click, drag to pan while zoomed, and the buttons and keys through `zoomIn` / `zoomOut` / `fit` / `toggle`. The photo
 * is moved with a CSS transform on `layer`, written straight to the element while a gesture runs; React only hears about the
 * two things the rest of the viewer needs: whether it is `zoomed` (the swipe to the details and paging are off then) and
 * whether the zoom went `wantSharp` (past what the preview shows: a sharper picture, if there is one, should be loaded).
 */
export function usePhotoZoom(opts: ZoomOptions) {
  const o = useRef(opts);
  useEffect(() => {
    o.current = opts;
  });
  const view = useRef<View>(FIT);
  const [zoomed, setZoomed] = useState(false);
  const [wantSharp, setWantSharp] = useState(false);
  const sharp = useRef(false);
  const timers = useRef<{ announce?: number; settle?: number }>({});

  /** The stage's and the fitted photo's sizes (layout pixels, not scaled), and what the zoom limits depend on. */
  const geometry = useCallback(() => {
    const s = o.current.stage.current;
    const p = o.current.photo.current;
    if (!s || !p || p.offsetWidth === 0) return null;
    const stage: Size = { w: s.clientWidth, h: s.clientHeight };
    const photo: Size = { w: p.offsetWidth, h: p.offsetHeight };
    const fitLong = Math.max(photo.w, photo.h);
    const dpr = window.devicePixelRatio || 1;
    return { stage, photo, fitLong, dpr, max: maxScale(o.current.naturalLong, fitLong, dpr) };
  }, []);

  const apply = useCallback((v: View, mode: "gesture" | "ease" | "jump") => {
    view.current = v;
    const layer = o.current.layer.current;
    if (layer) {
      layer.classList.toggle("easing", mode === "ease");
      if (mode === "gesture") layer.classList.add("gesturing");
      layer.style.transform = v.scale === 1 && v.x === 0 && v.y === 0 ? "" : `translate(${v.x}px, ${v.y}px) scale(${v.scale})`;
      layer.dataset.zoom = String(Math.round(v.scale * 1000) / 1000);
    }
    const label = percentLabel(v.scale);
    const level = o.current.level.current;
    if (level) level.textContent = label;
    window.clearTimeout(timers.current.announce);
    timers.current.announce = window.setTimeout(() => {
      const live = o.current.announce.current;
      if (live) live.textContent = label;
    }, 400);
    setZoomed(v.scale > 1.001);
    if (!sharp.current) {
      const g = geometry();
      if (g && pastPreview(v.scale, g.fitLong, o.current.previewLong, g.dpr)) {
        sharp.current = true;
        setWantSharp(true);
      }
    }
  }, [geometry]);

  const endGesturing = useCallback(() => o.current.layer.current?.classList.remove("gesturing"), []);

  // A new photo (or none) starts at fit.
  const { resetKey } = opts;
  useLayoutEffect(() => {
    sharp.current = false;
    setWantSharp(false);
    apply(FIT, "jump");
    endGesturing();
  }, [resetKey, apply, endGesturing]);

  const centre = useCallback((x: number, y: number): Point => {
    const r = o.current.stage.current?.getBoundingClientRect();
    return r ? { x: x - (r.left + r.width / 2), y: y - (r.top + r.height / 2) } : { x: 0, y: 0 };
  }, []);

  const zoomTo = useCallback(
    (scale: number, at: Point, mode: "gesture" | "ease") => {
      const g = geometry();
      if (!g) return;
      apply(zoomAt(view.current, scale, at, g.stage, g.photo, g.max), mode);
    },
    [geometry, apply],
  );

  const step = useCallback(
    (dir: 1 | -1) => {
      const g = geometry();
      if (g) zoomTo(stepScale(view.current.scale, dir, g.max), { x: 0, y: 0 }, "ease");
    },
    [geometry, zoomTo],
  );
  const zoomIn = useCallback(() => step(1), [step]);
  const zoomOut = useCallback(() => step(-1), [step]);
  const fit = useCallback(() => zoomTo(1, { x: 0, y: 0 }, "ease"), [zoomTo]);
  /** Fit <-> the double-tap zoom, around `at` (the stage's centre by default). */
  const toggle = useCallback(
    (at: Point = { x: 0, y: 0 }) => {
      const g = geometry();
      if (!g) return;
      if (view.current.scale > 1.001) zoomTo(1, at, "ease");
      else zoomTo(doubleTapScale(o.current.naturalLong, g.fitLong, g.dpr, g.max), at, "ease");
    },
    [geometry, zoomTo],
  );

  // Keep a zoomed photo on screen when the stage changes size (a panel opens, the phone turns).
  useEffect(() => {
    const s = opts.stage.current;
    if (!s || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      const g = geometry();
      if (g && view.current.scale > 1) apply(clampView(view.current, g.stage, g.photo, g.max), "jump");
    });
    ro.observe(s);
    return () => ro.disconnect();
  }, [opts.stage, opts.active, geometry, apply]);

  // Wheel with ctrl (a trackpad pinch, or ctrl + wheel) zooms the photo around the cursor; a plain wheel is left alone. Safari's
  // trackpad pinch arrives as gesture events instead. Both need listeners that may cancel the page's own zoom.
  useEffect(() => {
    const s = opts.stage.current;
    if (!s) return;
    const t = timers.current;
    const settle = () => {
      window.clearTimeout(t.settle);
      t.settle = window.setTimeout(endGesturing, 200);
    };
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      zoomTo(view.current.scale * wheelFactor(e.deltaY, e.deltaMode), centre(e.clientX, e.clientY), "gesture");
      settle();
    };
    let startScale = 1;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      startScale = view.current.scale;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const ge = e as Event & { scale: number; clientX: number; clientY: number };
      zoomTo(startScale * ge.scale, centre(ge.clientX, ge.clientY), "gesture");
      settle();
    };
    s.addEventListener("wheel", onWheel, { passive: false });
    s.addEventListener("gesturestart", onGestureStart);
    s.addEventListener("gesturechange", onGestureChange);
    return () => {
      s.removeEventListener("wheel", onWheel);
      s.removeEventListener("gesturestart", onGestureStart);
      s.removeEventListener("gesturechange", onGestureChange);
      window.clearTimeout(t.settle);
    };
  }, [opts.stage, opts.active, zoomTo, centre, endGesturing]);

  useEffect(
    () => () => {
      window.clearTimeout(timers.current.announce);
      window.clearTimeout(timers.current.settle);
    },
    [],
  );

  // Pointers: one pans (while zoomed) or taps, two pinch.
  const pointers = useRef(new Map<number, Point>());
  const mode = useRef<{ kind: "none" } | { kind: "pan"; start: View; from: Point } | { kind: "pinch"; start: View; from: [Point, Point] }>({ kind: "none" });
  const tap = useRef<{ id: number; x: number; y: number; t: number; moved: boolean } | null>(null);
  const lastTap = useRef<{ x: number; y: number; t: number } | null>(null);

  const startPan = (id: number) => {
    const p = pointers.current.get(id);
    if (p && view.current.scale > 1.001) mode.current = { kind: "pan", start: view.current, from: p };
    else mode.current = { kind: "none" };
  };
  const twoPoints = (): [Point, Point] | null => {
    const it = Array.from(pointers.current.values());
    return it.length >= 2 ? [it[0]!, it[1]!] : null;
  };
  const toCentre = (pair: [Point, Point]): [Point, Point] => [centre(pair[0].x, pair[0].y), centre(pair[1].x, pair[1].y)];

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button, a, input, select, textarea")) return;
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* the pointer is already gone */
    }
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.current.size === 1) {
      tap.current = { id: e.pointerId, x: e.clientX, y: e.clientY, t: e.timeStamp, moved: false };
      startPan(e.pointerId);
      if (mode.current.kind === "pan") o.current.layer.current?.classList.add("gesturing");
    } else if (pointers.current.size === 2) {
      tap.current = null;
      const pair = twoPoints()!;
      mode.current = { kind: "pinch", start: view.current, from: toCentre(pair) };
      o.current.layer.current?.classList.add("gesturing");
    }
  };

  const onPointerMove = (e: ReactPointerEvent) => {
    if (!pointers.current.has(e.pointerId)) return;
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    const t = tap.current;
    if (t && Math.hypot(e.clientX - t.x, e.clientY - t.y) > TAP_SLOP_PX) t.moved = true;
    const m = mode.current;
    const g = geometry();
    if (!g) return;
    if (m.kind === "pan" && pointers.current.size === 1) {
      apply(clampView({ ...m.start, x: m.start.x + e.clientX - m.from.x, y: m.start.y + e.clientY - m.from.y }, g.stage, g.photo, g.max), "gesture");
    } else if (m.kind === "pinch") {
      const pair = twoPoints();
      if (pair) apply(pinchView(m.start, m.from, toCentre(pair), g.stage, g.photo, g.max), "gesture");
    }
  };

  const end = (e: ReactPointerEvent, finished: boolean) => {
    if (!pointers.current.delete(e.pointerId)) return;
    const t = tap.current;
    if (t?.id === e.pointerId) tap.current = null;
    if (mode.current.kind === "pinch" && pointers.current.size < 2) {
      // One finger stays down: it carries on as a pan (if still zoomed in).
      const rest = pointers.current.keys().next();
      if (view.current.scale < SNAP_SCALE) apply(FIT, "ease");
      if (rest.done) mode.current = { kind: "none" };
      else startPan(rest.value);
    } else if (mode.current.kind === "pan" && pointers.current.size === 0) {
      mode.current = { kind: "none" };
    }
    if (pointers.current.size === 0) endGesturing();
    if (finished && t && t.id === e.pointerId && !t.moved && e.timeStamp - t.t < TAP_MS) {
      const prev = lastTap.current;
      if (prev && e.timeStamp - prev.t < TAP_MS && Math.hypot(e.clientX - prev.x, e.clientY - prev.y) < DOUBLE_TAP_SLOP_PX) {
        lastTap.current = null;
        toggle(centre(e.clientX, e.clientY));
      } else {
        lastTap.current = { x: e.clientX, y: e.clientY, t: e.timeStamp };
      }
    }
  };

  return {
    zoomed,
    wantSharp,
    zoomIn,
    zoomOut,
    fit,
    toggle,
    handlers: {
      onPointerDown,
      onPointerMove,
      onPointerUp: (e: ReactPointerEvent) => end(e, true),
      onPointerCancel: (e: ReactPointerEvent) => end(e, false),
    },
  };
}
