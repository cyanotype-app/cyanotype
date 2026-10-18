// Pure helpers of the photo details panel: the wording of the date, the coordinates and the shortcuts. The numbers
// themselves (exposure line, altitude...) are formatted by the server (apps/assets/src/details.rs) and shown as they come.
import type { PhotoDetails, SearchParams } from "./api";
import { urlFromParams } from "./searchText";

type DateInfo = PhotoDetails["date"];

const dayFmt = new Intl.DateTimeFormat("en-US", {
  weekday: "long",
  month: "long",
  day: "numeric",
  year: "numeric",
  timeZone: "UTC",
});
const timeFmt = new Intl.DateTimeFormat("en-US", { hour: "numeric", minute: "2-digit", timeZone: "UTC" });

/** `local` is a wall-clock time; formatting it "as UTC" prints exactly those digits whatever the browser's zone is. */
const wall = (local: string) => new Date(`${local.slice(0, 19)}Z`);

/** "UTC+02:00", "UTC" for +00:00, or null when the photo carries no zone. */
export function zoneText(offset: string | null): string | null {
  if (!offset) return null;
  return /^[+-]00:00$/.test(offset) ? "UTC" : `UTC${offset}`;
}

export interface WhenText {
  /** "Tuesday, July 4, 2023". */
  day: string;
  /** "1:05 PM". */
  time: string;
  /** "UTC+02:00", or "Time zone unknown" for a time EXIF gave no zone for. */
  zone: string;
  /** What the date is when it is not the camera's clock. */
  note: string | null;
}

export function whenText(d: DateInfo): WhenText {
  const t = wall(d.local);
  const valid = !Number.isNaN(t.getTime());
  return {
    day: valid ? dayFmt.format(t) : d.local.slice(0, 10),
    time: valid ? timeFmt.format(t) : "",
    zone: zoneText(d.offset) ?? "Time zone unknown",
    note:
      d.basis === "file"
        ? "No capture time in the file: this is its modification date."
        : d.basis === "added"
          ? "No capture time in the file: this is when it was added."
          : null,
  };
}

const hemisphere = (v: number, pos: string, neg: string) => `${Math.abs(v).toFixed(4)}° ${v < 0 ? neg : pos}`;

/** "38.7223° N, 9.1393° W". */
export const coordinatesText = (lat: number, lon: number) => `${hemisphere(lat, "N", "S")}, ${hemisphere(lon, "E", "W")}`;

/** "1 of 8 in a burst". */
export const burstText = (b: { position: number; size: number }) => `${b.position} of ${b.size} in a burst`;

/** "4032 × 3024 · 12 MP". */
export function resolutionText(f: Pick<PhotoDetails["file"], "width" | "height" | "megapixels">): string | null {
  if (!f.width || !f.height) return null;
  return [`${f.width} × ${f.height}`, f.megapixels].filter(Boolean).join(" · ");
}

/** The search page filtered by one thing (a camera, a lens, a place). */
export const searchHref = (p: SearchParams) => `/search?${urlFromParams(p)}`;

/** The map page centred on a photo. */
export const mapHref = (lat: number, lon: number) => `/map?${new URLSearchParams({ lat: String(lat), lon: String(lon) })}`;

/** The library scrolled to a day (`YYYY-MM-DD`). */
export const dayHref = (day: string) => `/library?${new URLSearchParams({ day })}`;

const DAY = /^\d{4}-\d{2}-\d{2}$/;
const num = (v: string | null) => (v !== null && v.trim() !== "" && Number.isFinite(Number(v)) ? Number(v) : null);

/** `?lat=&lon=` of the map page as a point, or null when absent or out of range. */
export function pointFromSearch(search: URLSearchParams): { lat: number; lon: number } | null {
  const lat = num(search.get("lat"));
  const lon = num(search.get("lon"));
  if (lat === null || lon === null || Math.abs(lat) > 90 || Math.abs(lon) > 180) return null;
  return { lat, lon };
}

/** `?day=` of the library as a day, or null. */
export const dayFromSearch = (search: URLSearchParams): string | null => {
  const v = search.get("day");
  return v && DAY.test(v) && !Number.isNaN(Date.parse(`${v}T00:00:00Z`)) ? v : null;
};
