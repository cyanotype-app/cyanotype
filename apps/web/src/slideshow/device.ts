// What the browser lets a slideshow do with the screen: full screen (not on an iPhone, where Safari has none for pages), keep the
// screen awake, and know that the person asked for less motion. Each is feature-detected and fails quietly.
import { useEffect, useState } from "react";

type FsDocument = Document & {
  webkitFullscreenElement?: Element | null;
  webkitExitFullscreen?: () => Promise<void> | void;
  webkitFullscreenEnabled?: boolean;
};
type FsElement = HTMLElement & { webkitRequestFullscreen?: () => Promise<void> | void };

/** The page can be made full screen (Chrome, Firefox, desktop and iPad Safari; not Safari on an iPhone). */
export function fullscreenSupported(): boolean {
  if (typeof document === "undefined") return false;
  const d = document as FsDocument;
  const el = document.documentElement as FsElement;
  if (typeof el.requestFullscreen !== "function" && typeof el.webkitRequestFullscreen !== "function") return false;
  return (d.fullscreenEnabled ?? d.webkitFullscreenEnabled) !== false;
}

export function isFullscreen(): boolean {
  const d = document as FsDocument;
  return !!(d.fullscreenElement ?? d.webkitFullscreenElement);
}

/** Ask for full screen. Must run in a click or key handler; never throws (the slideshow fills the window instead). */
export async function enterFullscreen(): Promise<boolean> {
  if (!fullscreenSupported() || isFullscreen()) return isFullscreen();
  const el = document.documentElement as FsElement;
  try {
    await (el.requestFullscreen ? el.requestFullscreen({ navigationUI: "hide" }) : el.webkitRequestFullscreen?.());
    return true;
  } catch {
    return false;
  }
}

export async function exitFullscreen(): Promise<void> {
  if (!isFullscreen()) return;
  const d = document as FsDocument;
  try {
    await (d.exitFullscreen ? d.exitFullscreen() : d.webkitExitFullscreen?.());
  } catch {
    /* already out */
  }
}

/** Whether the page is full screen now, following the browser (Escape leaves it without the page asking). */
export function useFullscreen() {
  const [active, setActive] = useState(() => (typeof document === "undefined" ? false : isFullscreen()));
  useEffect(() => {
    const on = () => setActive(isFullscreen());
    document.addEventListener("fullscreenchange", on);
    document.addEventListener("webkitfullscreenchange", on);
    return () => {
      document.removeEventListener("fullscreenchange", on);
      document.removeEventListener("webkitfullscreenchange", on);
    };
  }, []);
  return {
    supported: fullscreenSupported(),
    active,
    toggle: () => void (isFullscreen() ? exitFullscreen() : enterFullscreen()),
  };
}

/** Keep the screen from dimming while `active` (Screen Wake Lock; where it is missing nothing happens). */
export function useWakeLock(active: boolean) {
  useEffect(() => {
    if (!active || typeof navigator === "undefined" || !("wakeLock" in navigator)) return;
    let sentinel: WakeLockSentinel | null = null;
    let cancelled = false;
    const acquire = async () => {
      try {
        const s = await navigator.wakeLock.request("screen");
        if (cancelled) void s.release().catch(() => undefined);
        else sentinel = s;
      } catch {
        /* refused (low battery, background tab): the screen may dim */
      }
    };
    // A browser lets the lock go when the tab is hidden: take it again when the tab is back.
    const onVisible = () => {
      if (document.visibilityState === "visible" && (!sentinel || sentinel.released)) void acquire();
    };
    void acquire();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      cancelled = true;
      document.removeEventListener("visibilitychange", onVisible);
      void sentinel?.release().catch(() => undefined);
    };
  }, [active]);
}

const REDUCED = "(prefers-reduced-motion: reduce)";

export function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(REDUCED).matches;
}

export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(prefersReducedMotion);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(REDUCED);
    const on = () => setReduced(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return reduced;
}
