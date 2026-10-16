// An in-memory stand-in for the assets service's timeline, list, favorite and trash endpoints, so the Library page can
// be tested against realistic paging (counts per day, `before` and keyset cursors) instead of canned responses.
import { vi } from "vitest";
import { api, type AssetListItem } from "../api";
import { dayOfTime } from "../timeline";

const newestFirst = (a: AssetListItem, b: AssetListItem) =>
  Date.parse(b.sort_time) - Date.parse(a.sort_time) || (a.id < b.id ? 1 : a.id > b.id ? -1 : 0);

export interface FakeLibrary {
  /** The live photos (not trashed), mutable. */
  photos: AssetListItem[];
  trashed: AssetListItem[];
  listAssets: ReturnType<typeof vi.spyOn>;
  timeline: ReturnType<typeof vi.spyOn>;
}

/** Install spies on `api` that behave like the server for the given photos. */
export function fakeLibrary(initial: AssetListItem[]): FakeLibrary {
  const lib = { photos: [...initial], trashed: [] as AssetListItem[] } as FakeLibrary;
  const visible = (favorite?: boolean) =>
    lib.photos.filter((p) => !favorite || p.favorite === true).sort(newestFirst);

  lib.timeline = vi.spyOn(api, "timeline").mockImplementation(async (opts = {}) => {
    const counts = new Map<string, number>();
    for (const p of visible(opts.favorite)) counts.set(dayOfTime(p.sort_time), (counts.get(dayOfTime(p.sort_time)) ?? 0) + 1);
    const days = [...counts].map(([day, count]) => ({ day, count })).sort((a, b) => (a.day < b.day ? 1 : -1));
    return { total: days.reduce((n, d) => n + d.count, 0), days };
  });

  lib.listAssets = vi.spyOn(api, "listAssets").mockImplementation(async (opts = {}) => {
    let rows = visible(opts.favorite);
    if (opts.cursor) {
      const [t, id] = opts.cursor.split("|");
      rows = rows.filter((p) => Date.parse(p.sort_time) < Date.parse(t!) || (Date.parse(p.sort_time) === Date.parse(t!) && p.id < id!));
    } else if (opts.before) {
      rows = rows.filter((p) => Date.parse(p.sort_time) < Date.parse(opts.before!));
    }
    const limit = opts.limit ?? 100;
    const items = rows.slice(0, limit);
    const last = items.at(-1);
    const more = items.length === limit && last;
    return {
      items,
      next_cursor: more ? `${last.sort_time}|${last.id}` : null,
      next_before: more ? last.sort_time : null,
    };
  });

  vi.spyOn(api, "trashAssets").mockImplementation(async (ids) => {
    const gone = lib.photos.filter((p) => ids.includes(p.id));
    lib.photos = lib.photos.filter((p) => !ids.includes(p.id));
    lib.trashed.push(...gone);
    return { count: gone.length };
  });
  vi.spyOn(api, "restoreAssets").mockImplementation(async (ids) => {
    const back = lib.trashed.filter((p) => ids.includes(p.id));
    lib.trashed = lib.trashed.filter((p) => !ids.includes(p.id));
    lib.photos.push(...back);
    return { count: back.length };
  });
  vi.spyOn(api, "favoriteAssets").mockImplementation(async (ids, favorite) => {
    let count = 0;
    for (const p of lib.photos) {
      if (ids.includes(p.id) && (p.favorite === true) !== favorite) {
        p.favorite = favorite;
        count++;
      }
    }
    return { count };
  });
  return lib;
}
