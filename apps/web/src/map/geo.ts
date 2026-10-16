// Small pure helpers for the map page: the viewport as an API parameter, place text, where a cluster click goes.
import type { MapCluster, Place } from "../api";

/** The visible part of the map in degrees. `west` and `east` may lie outside -180..180 (the map was scrolled past the date line). */
export interface Bounds {
  west: number;
  south: number;
  east: number;
  north: number;
}

/** The whole world: what the overview asks for. */
export const WORLD: Bounds = { west: -180, south: -90, east: 180, north: 90 };

const deg = (x: number) => String(Math.round(x * 1e5) / 1e5);

/** `west,south,east,north` for the API (it wraps longitudes and treats a box 360 degrees wide as the world). */
export function bboxParam(b: Bounds): string {
  const south = Math.max(-90, Math.min(90, b.south));
  const north = Math.max(-90, Math.min(90, b.north));
  if (b.east - b.west >= 360) return `-180,${deg(south)},180,${deg(north)}`;
  return `${deg(b.west)},${deg(south)},${deg(b.east)},${deg(north)}`;
}

/** The zoom the API clusters at: whole levels, so panning at one level keeps the same grid. */
export const clusterZoom = (zoom: number) => Math.max(0, Math.min(22, Math.floor(zoom)));

/** "Paris, Île-de-France, France": the region is left out when it only repeats the city. */
export function placeLine(p: Place | null | undefined): string {
  if (!p) return "";
  const parts = [p.city];
  if (p.region && p.region !== p.city) parts.push(p.region);
  parts.push(p.country);
  return parts.join(", ");
}

export type ClusterTarget =
  | { kind: "bounds"; bounds: [[number, number], [number, number]] }
  | { kind: "center"; center: [number, number]; zoom: number };

/**
 * Where a click on a cluster goes: to fit its photos when they are spread out, else (all at one spot) a few levels
 * closer, which at the deepest zoom is as far as it goes.
 */
export function clusterTarget(c: MapCluster, zoom: number, maxZoom = 19): ClusterTarget {
  const b = c.bounds;
  if (b && (Math.abs(b[2] - b[0]) > 1e-6 || Math.abs(b[3] - b[1]) > 1e-6)) {
    return { kind: "bounds", bounds: [[b[0], b[1]], [b[2], b[3]]] };
  }
  return { kind: "center", center: [c.lon, c.lat], zoom: Math.min(maxZoom, Math.max(zoom + 2, 14)) };
}

/** The smallest box around the markers of a response (a cluster contributes all of its photos), or null when empty. */
export function extent(clusters: readonly MapCluster[]): Bounds | null {
  if (clusters.length === 0) return null;
  let west = 180;
  let east = -180;
  let south = 90;
  let north = -90;
  for (const c of clusters) {
    const [w, s, e, n] = c.bounds ?? [c.lon, c.lat, c.lon, c.lat];
    west = Math.min(west, w);
    east = Math.max(east, e);
    south = Math.min(south, s);
    north = Math.max(north, n);
  }
  return { west, south, east, north };
}

/** Marker bubble diameter in px for a cluster of `count` photos (grows slowly: 40 px for 2, about 64 px for 100,000). */
export const bubbleSize = (count: number) => Math.round(36 + Math.min(28, Math.log10(Math.max(1, count)) * 7 + 4));

/** "1.2k" for big counts so the bubble stays readable. */
export function shortCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(Math.round(n / 100) / 10).toString()}k`;
  return `${Math.round(n / 1000)}k`;
}
