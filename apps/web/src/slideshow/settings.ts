// A viewer's slideshow settings, kept in this browser only (localStorage may be missing or throw: a private window, blocked
// site data). Anything that does not parse falls back to the defaults, field by field.
import type { SlideOrder } from "../api";

export const DURATIONS = [3, 5, 10, 20] as const;
export const ORDERS: { value: SlideOrder; label: string }[] = [
  { value: "listed", label: "As listed" },
  { value: "oldest", label: "Oldest first" },
  { value: "shuffle", label: "Shuffle" },
];

export interface SlideshowSettings {
  /** Seconds each photo stays. */
  duration: number;
  order: SlideOrder;
  transition: "crossfade" | "none";
  loop: boolean;
}

export const DEFAULT_SETTINGS: SlideshowSettings = { duration: 5, order: "listed", transition: "crossfade", loop: true };

export const SETTINGS_KEY = "cyanotype.slideshow";

export function parseSettings(raw: unknown): SlideshowSettings {
  const o = typeof raw === "object" && raw !== null ? (raw as Record<string, unknown>) : {};
  const d = DEFAULT_SETTINGS;
  return {
    duration: typeof o.duration === "number" && (DURATIONS as readonly number[]).includes(o.duration) ? o.duration : d.duration,
    order: o.order === "listed" || o.order === "oldest" || o.order === "shuffle" ? o.order : d.order,
    transition: o.transition === "crossfade" || o.transition === "none" ? o.transition : d.transition,
    loop: typeof o.loop === "boolean" ? o.loop : d.loop,
  };
}

export function loadSettings(): SlideshowSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    return parseSettings(raw ? JSON.parse(raw) : null);
  } catch {
    return DEFAULT_SETTINGS;
  }
}

export function saveSettings(s: SlideshowSettings) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  } catch {
    /* private window or storage blocked: the settings last until the page closes */
  }
}
