// Pure helpers of star ratings: the words, the keyboard and the filter labels. Unit tested.

export const MAX_STARS = 5;

/** "Not rated", "1 star", "4 stars". */
export function starsLabel(n: number | undefined): string {
  const v = clampStars(n);
  return v === 0 ? "Not rated" : `${v} ${v === 1 ? "star" : "stars"}`;
}

/** 0 to 5 whole stars; anything else (absent, fractional, out of range) is read as the nearest valid rating. */
export function clampStars(n: number | undefined): number {
  if (n === undefined || !Number.isFinite(n)) return 0;
  return Math.max(0, Math.min(MAX_STARS, Math.round(n)));
}

/** The rating a key press means (`0` to `5`), or null: only a bare digit, never with a modifier. */
export function ratingKey(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">): number | null {
  if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return null;
  return /^[0-5]$/.test(e.key) ? Number(e.key) : null;
}

/** What tapping star `n` does when the photo has `current` stars: tapping the star it already has clears the rating. */
export function afterTap(current: number | undefined, n: number): number {
  return clampStars(current) === n ? 0 : n;
}

/** The toast after a bulk rating. */
export function ratedMessage(count: number, rating: number): string {
  const photos = `${count.toLocaleString("en-US")} ${count === 1 ? "photo" : "photos"}`;
  return rating === 0 ? `Rating cleared on ${photos}` : `${photos} rated ${starsLabel(rating)}`;
}

/** The filter chip: "4 stars & up" ("5 stars" is already the top). */
export function ratingFilterLabel(min: number): string {
  const v = clampStars(min);
  return v >= MAX_STARS ? "5 stars" : `${v} ${v === 1 ? "star" : "stars"} & up`;
}
