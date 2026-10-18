// Behaviour of the lightbox's Info panel that has nothing to do with what it shows: a history entry so the browser's
// Back button (and a phone's back gesture) closes the panel before it leaves the page, and a vertical swipe detector.
import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

const hasEntry = () => (window.history.state as { cyInfo?: boolean } | null)?.cyInfo === true;

/**
 * Open/closed state of the panel, backed by one history entry while it is open. `hide` goes back through history (so the
 * entry is not left behind); Back from the browser closes the panel by itself. `leave` pops the entry and resolves when
 * that has happened, for closing the lightbox or following a link without a stray entry in the history.
 */
export function useInfoHistory() {
  const [open, setOpen] = useState(false);

  const show = useCallback(() => {
    if (!hasEntry()) window.history.pushState({ ...(window.history.state as object | null), cyInfo: true }, "");
    setOpen(true);
  }, []);

  const hide = useCallback(() => {
    if (hasEntry()) window.history.back(); // the popstate listener below closes the panel
    else setOpen(false);
  }, []);

  const leave = useCallback(
    () =>
      new Promise<void>((done) => {
        if (!hasEntry()) return done();
        const on = () => {
          window.removeEventListener("popstate", on);
          done();
        };
        window.addEventListener("popstate", on);
        window.history.back();
      }),
    [],
  );

  const toggle = useCallback(() => (open ? hide() : show()), [open, hide, show]);

  useEffect(() => {
    const on = () => {
      if (!hasEntry()) setOpen(false);
    };
    window.addEventListener("popstate", on);
    return () => window.removeEventListener("popstate", on);
  }, []);

  // The lightbox went away with the panel open (the photo was trashed...): take the entry with it.
  useEffect(
    () => () => {
      if (hasEntry()) window.history.back();
    },
    [],
  );

  return { open, show, hide, leave, toggle };
}

/** Distance (px) of a deliberate swipe, and the shorter one that counts when it is quick. */
export const SWIPE_PX = 64;
export const FLICK_PX = 32;
export const FLICK_MS = 250;
/** Movement before the direction of a gesture is decided. */
const SLOP_PX = 10;

interface Gesture {
  id: number;
  x: number;
  y: number;
  t: number;
  axis: "v" | "h" | null;
}

/**
 * Pointer handlers that call `onUp` / `onDown` for a clear vertical swipe by one finger. They stay out of the way of
 * everything else on the surface: a mouse (the desktop has the Info button and `i`), buttons and links, a second finger
 * (pinch), a page already pinch-zoomed in, and mostly horizontal movement (paging). Give the surface
 * `touch-action: pinch-zoom` so the browser does not take vertical drags for scrolling.
 */
export function useVerticalSwipe({ onUp, onDown, enabled = true }: { onUp?: () => void; onDown?: () => void; enabled?: boolean }) {
  const gesture = useRef<Gesture | null>(null);
  const pointers = useRef(new Set<number>());
  const cb = useRef({ onUp, onDown });
  useEffect(() => {
    cb.current = { onUp, onDown };
  });

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.pointerType === "mouse") return;
    pointers.current.add(e.pointerId);
    if (pointers.current.size > 1) {
      gesture.current = null; // a second finger: this is a pinch
      return;
    }
    if (!enabled || (e.target as HTMLElement).closest("button, a, input, select, textarea")) return;
    if ((window.visualViewport?.scale ?? 1) > 1.02) return; // zoomed in: the drag pans the picture
    gesture.current = { id: e.pointerId, x: e.clientX, y: e.clientY, t: e.timeStamp, axis: null };
  };

  const onPointerMove = (e: ReactPointerEvent) => {
    const g = gesture.current;
    if (!g || g.id !== e.pointerId || g.axis) return;
    const dx = e.clientX - g.x;
    const dy = e.clientY - g.y;
    if (Math.abs(dx) < SLOP_PX && Math.abs(dy) < SLOP_PX) return;
    g.axis = Math.abs(dy) > Math.abs(dx) * 1.5 ? "v" : "h";
  };

  const end = (e: ReactPointerEvent, finished: boolean) => {
    pointers.current.delete(e.pointerId);
    const g = gesture.current;
    if (!g || g.id !== e.pointerId) return;
    gesture.current = null;
    if (!finished || g.axis !== "v") return;
    const dy = e.clientY - g.y;
    const quick = e.timeStamp - g.t <= FLICK_MS;
    if (dy <= -SWIPE_PX || (quick && dy <= -FLICK_PX)) cb.current.onUp?.();
    else if (dy >= SWIPE_PX || (quick && dy >= FLICK_PX)) cb.current.onDown?.();
  };

  return {
    onPointerDown,
    onPointerMove,
    onPointerUp: (e: ReactPointerEvent) => end(e, true),
    onPointerCancel: (e: ReactPointerEvent) => end(e, false),
  };
}
