// Pure helpers of the edit dialogs: the zone choices, the shift as seconds, and the request bodies.
import type { DateEdit } from "./api";

/** `+HH:MM` for every half hour from UTC-12:00 to UTC+14:00 (the zones in use, with some to spare). */
export const OFFSETS: string[] = (() => {
  const out: string[] = [];
  for (let m = -12 * 60; m <= 14 * 60; m += 30) {
    const a = Math.abs(m);
    out.push(`${m < 0 ? "-" : "+"}${String(Math.floor(a / 60)).padStart(2, "0")}:${String(a % 60).padStart(2, "0")}`);
  }
  return out;
})();

export const offsetLabel = (o: string) => (o === "+00:00" ? "UTC" : `UTC${o}`);

export interface Shift {
  later: boolean;
  days: number;
  hours: number;
  minutes: number;
}

const whole = (v: number) => (Number.isFinite(v) ? Math.max(0, Math.floor(v)) : 0);

/** The shift in seconds (negative for earlier); 0 when nothing is entered. */
export function shiftSeconds(s: Shift): number {
  const secs = whole(s.days) * 86_400 + whole(s.hours) * 3_600 + whole(s.minutes) * 60;
  return s.later ? secs : -secs;
}

/** "1 day 2 hours earlier" for the preview line. */
export function shiftText(s: Shift): string {
  const parts = ([[whole(s.days), "day"], [whole(s.hours), "hour"], [whole(s.minutes), "minute"]] as const)
    .filter(([v]) => v > 0)
    .map(([v, u]) => `${v} ${u}${v === 1 ? "" : "s"}`);
  return parts.length ? `${parts.join(" ")} ${s.later ? "later" : "earlier"}` : "no change";
}

/** A `datetime-local` value (`2023-07-04T13:05` or with seconds) and a zone (`+02:00`, or "" for unknown) to the request. */
export const setDate = (local: string, offset: string): DateEdit => ({ local, offset: offset || null });

/** `YYYY-MM-DDTHH:MM:SS` from the server to what `<input type="datetime-local" step="1">` takes. */
export const inputLocal = (local: string) => local.slice(0, 19);

/** Parse "38.7223, -9.1393" (or separated by a space or semicolon) into a point; null when it is not one. */
export function parsePoint(text: string): { latitude: number; longitude: number } | null {
  const m = /^\s*(-?\d+(?:\.\d+)?)\s*[,; ]\s*(-?\d+(?:\.\d+)?)\s*$/.exec(text);
  if (!m) return null;
  const latitude = Number(m[1]);
  const longitude = Number(m[2]);
  return Math.abs(latitude) <= 90 && Math.abs(longitude) <= 180 ? { latitude, longitude } : null;
}
