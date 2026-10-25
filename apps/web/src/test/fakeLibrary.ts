// An in-memory stand-in for the assets service's timeline, list, favorite and trash endpoints, so the Library page can
// be tested against realistic paging (counts per day, `before` and keyset cursors) instead of canned responses.
import { vi } from "vitest";
import { api, isScope, type AssetListItem, type LibraryView, type Target } from "../api";
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
  // The server's views: hidden photos only in the Hidden view, archived ones only in the Archive (and in Favorites).
  const visible = (v: LibraryView = {}) =>
    lib.photos
      .filter((p) => (v.hidden ? p.hidden === true : p.hidden !== true))
      .filter((p) => !v.favorite || p.favorite === true)
      .filter((p) => (v.archived ? p.archived === true : v.favorite || v.hidden || v.person !== undefined || p.archived !== true))
      .sort(newestFirst); // (the fake has no upload times: both orders use sort_time)

  lib.timeline = vi.spyOn(api, "timeline").mockImplementation(async (opts = {}) => {
    const counts = new Map<string, number>();
    for (const p of visible(opts)) counts.set(dayOfTime(p.sort_time), (counts.get(dayOfTime(p.sort_time)) ?? 0) + 1);
    const days = [...counts].map(([day, count]) => ({ day, count })).sort((a, b) => (a.day < b.day ? 1 : -1));
    return { total: days.reduce((n, d) => n + d.count, 0), days };
  });

  lib.listAssets = vi.spyOn(api, "listAssets").mockImplementation(async (opts = {}) => {
    let rows = visible(opts);
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

  // Explicit ids, or a scope resolved like the server does (the view minus `except`; a batch is what one trash took).
  const batches = new Map<string, Set<string>>();
  const pick = (t: Target, from: AssetListItem[]) => {
    if (!isScope(t)) return from.filter((p) => t.includes(p.id));
    const s = t.scope;
    const rows =
      s === "favorites"
        ? from.filter((p) => p.favorite === true && p.hidden !== true)
        : s === "archive"
          ? from.filter((p) => p.archived === true && p.hidden !== true)
          : s === "hidden"
            ? from.filter((p) => p.hidden === true)
            : typeof s === "object" && "person" in s
              ? from.filter((p) => p.hidden !== true) // (every photo of the fake belongs to the person under test)
            : s === "library"
              ? from.filter((p) => p.hidden !== true && p.archived !== true)
              : typeof s === "object" && "trash_batch" in s
          ? from.filter((p) => batches.get(s.trash_batch)?.has(p.id))
          : from;
    return rows.filter((p) => !t.except.includes(p.id));
  };
  vi.spyOn(api, "trashAssets").mockImplementation(async (target) => {
    const gone = pick(target, lib.photos);
    const ids = new Set(gone.map((p) => p.id));
    lib.photos = lib.photos.filter((p) => !ids.has(p.id));
    lib.trashed.push(...gone);
    const batch = isScope(target) ? `batch-${batches.size + 1}` : undefined;
    if (batch) batches.set(batch, ids);
    return { count: gone.length, batch };
  });
  vi.spyOn(api, "restoreAssets").mockImplementation(async (target) => {
    const back = pick(target, lib.trashed);
    const ids = new Set(back.map((p) => p.id));
    lib.trashed = lib.trashed.filter((p) => !ids.has(p.id));
    lib.photos.push(...back);
    return { count: back.length };
  });
  vi.spyOn(api, "favoriteAssets").mockImplementation(async (target, favorite) => {
    let count = 0;
    for (const p of pick(target, lib.photos)) {
      if ((p.favorite === true) !== favorite) {
        p.favorite = favorite;
        count++;
      }
    }
    return { count };
  });
  // The per-user switches and the strip: defaults, and nothing for today.
  vi.spyOn(api, "prefs").mockResolvedValue({ memories_enabled: true, library_sort: "taken" });
  vi.spyOn(api, "memories").mockResolvedValue({ enabled: true, day: "01-01", years: [] });
  vi.spyOn(api, "archiveAssets").mockImplementation(async (target, archived) => {
    const rows = pick(target, lib.photos);
    for (const p of rows) p.archived = archived;
    return { count: rows.length };
  });
  vi.spyOn(api, "hideAssets").mockImplementation(async (target, hidden) => {
    const rows = pick(target, lib.photos);
    for (const p of rows) p.hidden = hidden;
    return { count: rows.length };
  });
  return lib;
}
