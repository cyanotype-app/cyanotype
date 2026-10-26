import { useEffect, useState } from "react";

const find = () => (typeof document === "undefined" ? null : document.querySelector<HTMLElement>("header.sticky"));

/** Height in px of the page's sticky header (what sticky headers and fixed rails sit below); 0 where there is none. */
export function headerHeight(): number {
  const el = find();
  return el ? el.getBoundingClientRect().height : 0;
}

/** `headerHeight`, following the header as it changes (a second row, a different width). */
export function useHeaderHeight(): number {
  const [h, setH] = useState(() => headerHeight());
  useEffect(() => {
    const el = find();
    const measure = () => setH(Math.round(headerHeight()));
    measure();
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return h;
}
