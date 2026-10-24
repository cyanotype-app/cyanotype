import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import type { PhotoText } from "./api";
import type { TextState } from "./components/TextPanel";

/**
 * The lightbox's Text panel: open or closed, whether boxes are drawn on the photo, and the text of the current photo, read
 * when the panel is opened (and again for each photo paged to while it is open). `load` undefined: no Text button.
 */
export function useTextPanel(load: ((id: string) => Promise<PhotoText>) | undefined, currentId: string | undefined) {
  const [open, setOpen] = useState(false);
  const [boxes, setBoxes] = useState(true);
  const [byId, setById] = useState<Record<string, PhotoText>>({});
  const [failed, setFailed] = useState<Record<string, true>>({});
  const requested = useRef(new Set<string>());
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    if (!open || !load || !currentId || requested.current.has(currentId)) return;
    requested.current.add(currentId);
    const id = currentId;
    load(id)
      .then((t) => setById((m) => ({ ...m, [id]: t })))
      .catch(() => {
        requested.current.delete(id);
        setFailed((m) => ({ ...m, [id]: true }));
      });
  }, [open, load, currentId, retry]);

  const current = currentId ? (byId[currentId] ?? null) : null;
  const state: TextState = current
    ? { status: "ready", text: current }
    : currentId && failed[currentId]
      ? { status: "error" }
      : { status: "loading" };
  const doRetry = useCallback(() => {
    if (!currentId) return;
    setFailed((m) => Object.fromEntries(Object.entries(m).filter(([k]) => k !== currentId)));
    setRetry((n) => n + 1);
  }, [currentId]);

  return {
    enabled: !!load,
    open,
    toggle: useCallback(() => setOpen((o) => !o), []),
    show: useCallback(() => setOpen(true), []),
    close: useCallback(() => setOpen(false), []),
    boxes,
    setBoxes,
    state,
    current,
    retry: doRetry,
  };
}

/** Where the displayed photo sits inside its stage (px), so boxes drawn in percentages line up with the picture. */
export function useImageRect(stage: RefObject<HTMLElement | null>, img: RefObject<HTMLImageElement | null>, deps: unknown[]) {
  const [rect, setRect] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  useEffect(() => {
    const s = stage.current;
    const i = img.current;
    if (!s || !i) {
      setRect(null);
      return;
    }
    const measure = () => {
      const sb = s.getBoundingClientRect();
      const ib = i.getBoundingClientRect();
      const next = ib.width > 0 ? { left: ib.left - sb.left, top: ib.top - sb.top, width: ib.width, height: ib.height } : null;
      setRect((prev) =>
        prev && next && Math.abs(prev.left - next.left) < 0.5 && Math.abs(prev.top - next.top) < 0.5 && Math.abs(prev.width - next.width) < 0.5 && Math.abs(prev.height - next.height) < 0.5
          ? prev
          : next,
      );
    };
    measure();
    // The picture can move without resizing (the preview replacing the thumbnail, a panel sliding in): look again while it matters.
    const timer = window.setInterval(measure, 250);
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(s);
    ro?.observe(i);
    window.addEventListener("resize", measure);
    return () => {
      window.clearInterval(timer);
      ro?.disconnect();
      window.removeEventListener("resize", measure);
    };
    // The caller lists what moves the picture (the photo, the panels, its loading).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return rect;
}
