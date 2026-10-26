// Words for the Collections page and the trip page.
import type { Album, Person, Trip } from "../api";
import { isNamed } from "../peopleText";
import { photoCount } from "../albumText";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function parts(day: string): { y: number; m: number; d: number } {
  const [y = 0, m = 1, d = 1] = day.split("-").map(Number);
  return { y, m: m - 1, d };
}

/** "Oct 12 – 19, 2026", "Sep 28 – Oct 3, 2026", "Dec 30, 2025 – Jan 2, 2026"; a one-day trip is just "Oct 12, 2026". */
export function dateRange(from: string, to: string): string {
  const a = parts(from);
  const b = parts(to);
  const one = (p: typeof a) => `${MONTHS[p.m]} ${p.d}, ${p.y}`;
  if (from === to) return one(a);
  if (a.y !== b.y) return `${one(a)} – ${one(b)}`;
  if (a.m === b.m) return `${MONTHS[a.m]} ${a.d} – ${b.d}, ${a.y}`;
  return `${MONTHS[a.m]} ${a.d} – ${MONTHS[b.m]} ${b.d}, ${a.y}`;
}

/** The line under a trip's title: when, then how many photos. */
export const tripLine = (t: Pick<Trip, "date_label" | "photos">) => `${t.date_label} · ${photoCount(t.photos)}`;

/** The card's name for assistive technology and the link's tooltip: "New York, October 2026, 124 photos". */
export const tripName = (t: Pick<Trip, "title" | "date_label" | "photos">) => `${t.title}, ${t.date_label}, ${photoCount(t.photos)}`;

/** Days between two `YYYY-MM-DD` dates (b - a), by the calendar. */
function daysBetween(a: string, b: string): number {
  const x = parts(a);
  const y = parts(b);
  return Math.round((Date.UTC(y.y, y.m, y.d) - Date.UTC(x.y, x.m, x.d)) / 86_400_000);
}

/** A trip worth a big card among the memories: still going, or over less than six weeks ago. */
export function isFreshTrip(t: Pick<Trip, "ongoing" | "to">, today: string): boolean {
  return t.ongoing || daysBetween(t.to, today) <= 42;
}

/** An album's count under its name; a smart album that matches more than is counted says so. */
export const albumCount = (a: Pick<Album, "photo_count" | "photo_count_capped">) =>
  a.photo_count_capped ? `${a.photo_count.toLocaleString("en-US")}+ photos` : photoCount(a.photo_count);

/** People for the home row: shown people who have a face to show, named ones first, then by how many photos. */
export function homePeople(people: readonly Person[], limit = 16): Person[] {
  const shown = people.filter((p) => !p.hidden && p.cover);
  const named = (p: Person) => (isNamed(p) ? 0 : 1);
  return shown.sort((a, b) => named(a) - named(b) || b.photos - a.photos).slice(0, limit);
}
