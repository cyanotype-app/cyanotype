import type { Processing } from "./api";

const KIND_LABEL: Record<string, string> = {
  metadata: "Reading details",
  thumbnail: "Making thumbnail",
  pair: "Pairing RAW + JPEG",
  pets: "Finding pets",
};

export const kindLabel = (kind: string) => KIND_LABEL[kind] ?? kind;

const n = (v: number) => v.toLocaleString("en-US");

/** The one-line status: "Processing 1,234 photos…" while thumbnails are pending, else a count of remaining tasks. */
export function processingLabel(p: Processing): string {
  if (p.without_thumbnail > 0) return `Processing ${n(p.without_thumbnail)} ${p.without_thumbnail === 1 ? "photo" : "photos"}…`;
  const tasks = p.queued + p.running;
  if (tasks > 0) return `Finishing ${n(tasks)} ${tasks === 1 ? "task" : "tasks"}…`;
  return "";
}

/** 0..100 of the library that has its thumbnail, or null when there is nothing to measure. */
export function processingPercent(p: Processing): number | null {
  if (p.without_thumbnail <= 0 || p.total_photos <= 0) return null;
  return Math.max(0, Math.min(99, Math.floor((1 - p.without_thumbnail / p.total_photos) * 100)));
}

/** Milliseconds until the next poll: quick while work is going on, slower and slower when idle. */
export function nextPollMs(active: boolean, idlePolls: number): number {
  if (active) return 3000;
  return Math.min(60_000, 10_000 * 2 ** Math.min(idlePolls, 3));
}
