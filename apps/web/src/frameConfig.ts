// What the /frame view shows, as the address holds it (so a wall tablet's start page is a bookmark) or as this browser last
// remembered it. Nothing here touches the network.
import type { Scope } from "./api";

export type FrameSource = { kind: "library" } | { kind: "favorites" } | { kind: "album"; id: string };

export interface FrameConfig {
  source: FrameSource;
  /** Seconds each photo stays. */
  interval: number;
  clock: boolean;
  caption: boolean;
}

export const MIN_INTERVAL = 5;
export const MAX_INTERVAL = 3600;
export const DEFAULT_INTERVAL = 30;
/** The choices the setup screen offers (seconds). */
export const INTERVALS = [10, 30, 60, 300, 900, 3600] as const;

export const FRAME_KEY = "cyanotype.frame";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const FLAG = (v: string | null) => v === "1" || v === "true";

export const frameScope = (s: FrameSource): Scope =>
  s.kind === "album" ? { album: s.id } : s.kind === "favorites" ? "favorites" : "library";

export const clampInterval = (n: number) => (Number.isFinite(n) ? Math.min(MAX_INTERVAL, Math.max(MIN_INTERVAL, Math.round(n))) : DEFAULT_INTERVAL);

/**
 * The configuration an address asks for: `?album=<id>` or `?source=library|favorites`, `interval=<seconds>`, `clock=1`,
 * `caption=1`. Null when it names none of them (the setup screen then asks), or names an album that is not an id.
 */
export function parseFrameParams(q: URLSearchParams): FrameConfig | null {
  const album = q.get("album");
  const source = q.get("source");
  if (album === null && source === null) return null;
  let src: FrameSource;
  if (album !== null) {
    if (!UUID.test(album)) return null;
    src = { kind: "album", id: album.toLowerCase() };
  } else if (source === "favorites") src = { kind: "favorites" };
  else if (source === "library") src = { kind: "library" };
  else return null;
  const iv = q.get("interval");
  return {
    source: src,
    interval: iv === null ? DEFAULT_INTERVAL : clampInterval(Number(iv)),
    clock: FLAG(q.get("clock")),
    caption: FLAG(q.get("caption")),
  };
}

export function frameParams(c: FrameConfig): URLSearchParams {
  const q = new URLSearchParams();
  if (c.source.kind === "album") q.set("album", c.source.id);
  else q.set("source", c.source.kind);
  q.set("interval", String(c.interval));
  if (c.clock) q.set("clock", "1");
  if (c.caption) q.set("caption", "1");
  return q;
}

export function loadFrameConfig(): FrameConfig | null {
  try {
    const raw = localStorage.getItem(FRAME_KEY);
    return raw ? parseFrameParams(new URLSearchParams(raw)) : null;
  } catch {
    return null;
  }
}

export function saveFrameConfig(c: FrameConfig) {
  try {
    localStorage.setItem(FRAME_KEY, frameParams(c).toString());
  } catch {
    /* storage blocked: the address still carries the choice */
  }
}
