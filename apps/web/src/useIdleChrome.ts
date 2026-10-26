import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

/** The viewer's bars fade out after this long without a touch, a mouse move or a key. */
export const IDLE_MS = 3000;

/**
 * Whether the viewer's chrome (bars and buttons) shows. It shows on any interaction (`show`), fades after a quiet spell, and
 * a tap on the photo toggles it. It never fades while `hold` (a panel is open, a photo is being edited), while a menu is
 * open, or while the keyboard is on one of the chrome's buttons.
 */
export function useIdleChrome(root: RefObject<HTMLElement | null>, hold: boolean) {
  const [visible, setVisible] = useState(true);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const holdRef = useRef(hold);
  useEffect(() => {
    holdRef.current = hold;
  });

  const arm = useCallback(() => {
    clearTimeout(timer.current);
    timer.current = setTimeout(function fade() {
      const busy = holdRef.current || !!document.querySelector('[role="menu"]') || !!root.current?.querySelector("[data-chrome] :focus-visible");
      if (busy) timer.current = setTimeout(fade, IDLE_MS);
      else setVisible(false);
    }, IDLE_MS);
  }, [root]);

  const show = useCallback(() => {
    setVisible(true);
    arm();
  }, [arm]);

  const toggle = useCallback(() => {
    setVisible((v) => {
      if (!v) arm();
      else clearTimeout(timer.current);
      return !v;
    });
  }, [arm]);

  useEffect(() => {
    arm();
    return () => clearTimeout(timer.current);
  }, [arm]);

  return { visible: visible || hold, show, toggle };
}
