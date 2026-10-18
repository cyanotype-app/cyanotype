// Bursts in search results: rapid-fire shots of one moment arrive collapsed behind a cover (`SearchItem.burst`). Pure helpers
// for what the page shows, what "select" and "move to trash" mean for a collapsed group, and keeping the groups right when a
// photo changes or goes. Kept apart from the components so the rules are unit tested.
import type { AssetListItem, BurstMember, SearchItem } from "./api";

type Result = SearchItem;

/** The photos a burst hides behind its cover. */
export const memberCount = (it: Result) => it.burst?.members.length ?? 0;

/** Every photo of the results, covers and members, in order: what "select all" and the lightbox's photos mean. */
export function allPhotos(items: readonly Result[]): AssetListItem[] {
  return items.flatMap((it) => [it, ...(it.burst?.members ?? [])]);
}

/** What the grid shows: a cover, and its members right behind it while it is expanded. */
export function visiblePhotos(items: readonly Result[], expanded: ReadonlySet<string>): AssetListItem[] {
  return items.flatMap((it) => (it.burst && expanded.has(it.id) ? [it, ...it.burst.members] : [it]));
}

/** The ids a click on this tile selects: the whole burst while it is collapsed, the photo alone otherwise. */
export function selectionIds(it: Result, expanded: ReadonlySet<string>): string[] {
  return it.burst && !expanded.has(it.id) ? [it.id, ...it.burst.members.map((m) => m.id)] : [it.id];
}

/** Covers and members of the results by id (members point at the cover whose burst they are in). */
export function indexResults(items: readonly Result[]): Map<string, { item: Result | BurstMember; cover: Result }> {
  const m = new Map<string, { item: Result | BurstMember; cover: Result }>();
  for (const it of items) {
    m.set(it.id, { item: it, cover: it });
    for (const member of it.burst?.members ?? []) m.set(member.id, { item: member, cover: it });
  }
  return m;
}

/** How many photos are hidden in collapsed bursts, and in how many bursts (for the selection bar's note). */
export function collapsedStats(items: readonly Result[], expanded: ReadonlySet<string>): { photos: number; bursts: number } {
  let photos = 0;
  let bursts = 0;
  for (const it of items) {
    if (it.burst && !expanded.has(it.id)) {
      bursts += 1;
      photos += it.burst.members.length;
    }
  }
  return { photos, bursts };
}

/** Change one photo, wherever it is (a cover or a member). */
export function patchPhoto<T extends Result>(items: readonly T[], id: string, patch: Partial<AssetListItem>): T[] {
  return items.map((it) => {
    if (it.id === id) return { ...it, ...patch };
    if (!it.burst || !it.burst.members.some((m) => m.id === id)) return it;
    return { ...it, burst: { ...it.burst, members: it.burst.members.map((m) => (m.id === id ? { ...m, ...patch } : m)) } };
  });
}

/**
 * Take photos out of the results (trashed). A burst that loses its cover is shown by its next photo; one that is down to a
 * single photo is just that photo.
 */
export function removePhotos<T extends Result>(items: readonly T[], gone: ReadonlySet<string>): T[] {
  const out: T[] = [];
  for (const it of items) {
    if (!it.burst) {
      if (!gone.has(it.id)) out.push(it);
      continue;
    }
    const alive: (Result | BurstMember)[] = [it, ...it.burst.members].filter((p) => !gone.has(p.id));
    const [cover, ...rest] = alive;
    if (!cover) continue;
    // The cover keeps its own fields; a promoted member brings its score and the cover's tier.
    const shown: SearchItem = cover === it ? it : { ...cover, tier: it.tier, score: (cover as BurstMember).score };
    const bare: SearchItem = { ...shown };
    delete bare.burst;
    out.push({ ...bare, ...(rest.length > 0 ? { burst: { size: alive.length, members: rest as BurstMember[] } } : {}) } as T);
  }
  return out;
}
