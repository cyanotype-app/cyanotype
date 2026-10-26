import { useSyncExternalStore } from "react";

/** Whether a media query matches, following changes (false where `matchMedia` is missing, as in some tests). */
export function useMedia(query: string): boolean {
  return useSyncExternalStore(
    (onChange) => {
      if (typeof window.matchMedia !== "function") return () => {};
      const m = window.matchMedia(query);
      m.addEventListener("change", onChange);
      return () => m.removeEventListener("change", onChange);
    },
    () => typeof window.matchMedia === "function" && window.matchMedia(query).matches,
    () => false,
  );
}

/** The desktop layout (sidebar, centred dialogs) from 768 px; below it the phone layout (tab bar, bottom sheets). */
export function useDesktop(): boolean {
  return useMedia("(min-width: 48rem)");
}
