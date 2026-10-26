// The viewer's one-finger gestures on the photo: drag sideways to page (the next photo follows the finger), drag down to
// close (the photo shrinks with the drag), drag up to open the info. The hook only watches and reports; the viewer draws.
import { useEffect, useRef, type PointerEvent as ReactPointerEvent } from "react";

/** Movement before the direction of a gesture is decided. */
const SLOP_PX = 8;

export interface DragMove {
  axis: "h" | "v";
  dx: number;
  dy: number;
}

export interface DragEnd extends DragMove {
  /** How long the finger was down (ms): a short drag is a flick and counts for less distance. */
  ms: number;
  /** The gesture was interrupted (a second finger, the system): settle back rather than commit. */
  cancelled: boolean;
}

interface Gesture {
  id: number;
  x: number;
  y: number;
  t: number;
  axis: "h" | "v" | null;
  /** Where the finger was last (a second finger ends the gesture there). */
  lx: number;
  ly: number;
}

/** Whether a drag of `d` px that took `ms` is a flick: quick and at least `min` px. */
export const isFlick = (d: number, ms: number, min = 32) => ms <= 250 && Math.abs(d) >= min;

/**
 * Pointer handlers for a touch or pen drag on the photo's area. They stay out of the way of everything else on the surface: a
 * mouse (the desktop pages with the arrows and closes with Escape), buttons and links, a second finger (that is a pinch), a
 * page already pinch-zoomed in, and a photo that is zoomed (the drag pans it). Give the surface `touch-action: none`.
 */
export function useViewerDrag({
  enabled,
  onMove,
  onEnd,
}: {
  enabled: boolean;
  onMove: (m: DragMove) => void;
  onEnd: (e: DragEnd) => void;
}) {
  const gesture = useRef<Gesture | null>(null);
  const pointers = useRef(new Set<number>());
  const cb = useRef({ onMove, onEnd });
  useEffect(() => {
    cb.current = { onMove, onEnd };
  });

  const finish = (g: Gesture, x: number, y: number, t: number, cancelled: boolean) => {
    gesture.current = null;
    if (!g.axis) return;
    cb.current.onEnd({ axis: g.axis, dx: x - g.x, dy: y - g.y, ms: t - g.t, cancelled });
  };

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.pointerType === "mouse") return;
    pointers.current.add(e.pointerId);
    if (pointers.current.size > 1) {
      // A second finger: this is a pinch.
      const g = gesture.current;
      if (g) finish(g, g.lx, g.ly, e.timeStamp, true);
      return;
    }
    if (!enabled || (e.target as HTMLElement).closest("button, a, input, select, textarea")) return;
    if ((window.visualViewport?.scale ?? 1) > 1.02) return; // zoomed in: the drag pans the picture
    gesture.current = { id: e.pointerId, x: e.clientX, y: e.clientY, t: e.timeStamp, axis: null, lx: e.clientX, ly: e.clientY };
  };

  const onPointerMove = (e: ReactPointerEvent) => {
    const g = gesture.current;
    if (!g || g.id !== e.pointerId) return;
    const dx = e.clientX - g.x;
    const dy = e.clientY - g.y;
    if (!g.axis) {
      if (Math.abs(dx) < SLOP_PX && Math.abs(dy) < SLOP_PX) return;
      g.axis = Math.abs(dy) > Math.abs(dx) * 1.3 ? "v" : "h";
    }
    g.lx = e.clientX;
    g.ly = e.clientY;
    cb.current.onMove({ axis: g.axis, dx, dy });
  };

  const end = (e: ReactPointerEvent, cancelled: boolean) => {
    pointers.current.delete(e.pointerId);
    const g = gesture.current;
    if (!g || g.id !== e.pointerId) return;
    finish(g, e.clientX, e.clientY, e.timeStamp, cancelled);
  };

  return {
    onPointerDown,
    onPointerMove,
    onPointerUp: (e: ReactPointerEvent) => end(e, false),
    onPointerCancel: (e: ReactPointerEvent) => end(e, true),
  };
}

/** Rubber band: the drag past an end (no photo there) moves a third as far. */
export const rubber = (dx: number) => dx * 0.33;
