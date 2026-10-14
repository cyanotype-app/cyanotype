// Wording for how long a trashed photo has left.
const DAY_MS = 24 * 60 * 60 * 1000;

const days = (n: number) => `${n} day${n === 1 ? "" : "s"}`;

/** "today", "yesterday", "3 days ago" for a past timestamp. */
export function daysAgo(iso: string, now: number): string {
  const n = Math.floor((now - Date.parse(iso)) / DAY_MS);
  if (!(n >= 1)) return "today";
  return n === 1 ? "yesterday" : `${days(n)} ago`;
}

/** "in 27 days", "tomorrow", "today" for a future timestamp (rounded up: 26.2 days left is "in 27 days"). */
export function daysUntil(iso: string, now: number): string {
  const n = Math.ceil((Date.parse(iso) - now) / DAY_MS);
  if (!(n >= 1)) return "today";
  return n === 1 ? "tomorrow" : `in ${days(n)}`;
}

/** Caption for a tile in the trash. */
export function trashCaption(item: { deleted_at: string; purge_at: string }, now = Date.now()): string {
  return `Deleted ${daysAgo(item.deleted_at, now)} · gone ${daysUntil(item.purge_at, now)}`;
}
