// Words for the library's views (All photos, Favorites, Recently added, Archive, Hidden) and for memories.
import type { MemoryYear } from "./api";

export type ViewKind = "library" | "favorites" | "recent" | "archive" | "hidden";

/** The visible heading of a view that is not one of the three tabs; null for those (they keep the invisible one). */
export function viewTitle(kind: ViewKind): string | null {
  switch (kind) {
    case "recent":
      return "Recently added";
    case "archive":
      return "Archive";
    case "hidden":
      return "Hidden";
    default:
      return null;
  }
}

/** What an empty view says; null for All photos, which offers uploads instead. */
export function emptyView(kind: ViewKind): { title: string; body: string } | null {
  switch (kind) {
    case "favorites":
      return {
        title: "No favorites yet",
        body: "Tap the heart on a photo, or select photos and choose Favorite, and they will be here.",
      };
    case "archive":
      return {
        title: "The archive is empty",
        body: "Archive photos you want to keep but not see in the timeline. They stay in albums and in search, and you restore them here.",
      };
    case "hidden":
      return {
        title: "No hidden photos",
        body: "Select photos and choose Hide, or open a photo and choose Hide. They leave every view until you open this one with your password.",
      };
    case "recent":
      return { title: "Nothing added yet", body: "Photos you upload show up here, newest first." };
    default:
      return null;
  }
}

const n = (v: number) => v.toLocaleString("en-US");

/** The toast after archiving or restoring. */
export function archiveMessage(count: number, archived: boolean): string {
  return archived ? `${n(count)} archived` : `${n(count)} back in the library`;
}

/** "1 year ago", "3 years ago". */
export const yearsAgo = (years: number) => (years === 1 ? "1 year ago" : `${years} years ago`);

/** "9 photos" under a memory year. */
export const memoryCount = (count: number) => `${n(count)} photo${count === 1 ? "" : "s"}`;

/** Does the strip have anything to show? An empty strip draws nothing at all. */
export const hasMemories = (years: readonly MemoryYear[]) => years.some((y) => y.items.length > 0);

/** The 10-minute Hidden view: "Unlocked until 14:05". */
export function unlockedUntil(expiresAt: string, locale?: string): string {
  const d = new Date(expiresAt);
  return `Unlocked until ${d.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" })}`;
}

/** Milliseconds until the unlock runs out (never negative). */
export const msUntil = (expiresAt: string, now = Date.now()) => Math.max(0, new Date(expiresAt).getTime() - now);
