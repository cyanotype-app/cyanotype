import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { PreviewCache } from "./previews";
import { DEFAULT_SETTINGS, SETTINGS_KEY, loadSettings, parseSettings, saveSettings } from "./settings";
import { MAX_LOCAL, arrange, itemsSource, pagedSource, scopeSource, searchSource } from "./sources";
import { photos } from "./testKit";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("scopeSource", () => {
  it("pages the server's feed with its cursor, a pass at a time", async () => {
    const page = vi
      .spyOn(api, "slideshowPage")
      .mockResolvedValueOnce({ items: photos(2) as never, next_cursor: "2", total: 3 })
      .mockResolvedValueOnce({ items: photos(1, "q") as never, next_cursor: null, total: 3 });
    const feed = scopeSource({ album: "a1" }, "Trip").open("shuffle", "seedy", 0);
    const first = await feed.next();
    expect(first).toMatchObject({ done: false, total: 3 });
    expect(page).toHaveBeenLastCalledWith({ album: "a1" }, { order: "shuffle", seed: "seedy", cursor: null, limit: 40 });
    const second = await feed.next();
    expect(second.done).toBe(true);
    expect(page).toHaveBeenLastCalledWith({ album: "a1" }, { order: "shuffle", seed: "seedy", cursor: "2", limit: 40 });
    expect((await feed.next()).items).toEqual([]);
    expect(page).toHaveBeenCalledTimes(2);
  });

  it("a failed page is asked for again from the same place", async () => {
    const page = vi
      .spyOn(api, "slideshowPage")
      .mockResolvedValueOnce({ items: photos(2) as never, next_cursor: "2", total: 4 })
      .mockRejectedValueOnce(new Error("down"))
      .mockResolvedValueOnce({ items: photos(2, "q") as never, next_cursor: null, total: 4 });
    const feed = scopeSource("library", "x").open("listed", "", 0);
    await feed.next();
    await expect(feed.next()).rejects.toThrow("down");
    expect((await feed.next()).items.map((i) => i.id)).toEqual(["q1", "q2"]);
    expect(page.mock.calls[2]![1]).toMatchObject({ cursor: "2" });
  });
});

describe("pagedSource", () => {
  const pages = (n: number, size: number) => {
    const all = photos(n);
    const fetchPage = vi.fn(async (cursor: string | null) => {
      const at = cursor ? Number(cursor) : 0;
      return { items: all.slice(at, at + size), next_cursor: at + size < n ? String(at + size) : null };
    });
    return { all, fetchPage };
  };

  it("as listed it reads one page at a time", async () => {
    const { fetchPage } = pages(25, 10);
    const feed = pagedSource(fetchPage, "s").open("listed", "", 0);
    expect((await feed.next()).items).toHaveLength(10);
    expect(fetchPage).toHaveBeenCalledTimes(1);
    expect((await feed.next()).done).toBe(false);
    expect((await feed.next()).done).toBe(true);
  });

  it("oldest first and shuffle read it all, then order it here", async () => {
    const { fetchPage, all } = pages(25, 10);
    const oldest = await pagedSource(fetchPage, "s").open("oldest", "", 0).next();
    expect(fetchPage).toHaveBeenCalledTimes(3);
    expect(oldest.items.map((i) => i.id)).toEqual(all.map((i) => i.id).reverse());
    const shuffled = await pagedSource(fetchPage, "s").open("shuffle", "k", 0).next();
    expect(new Set(shuffled.items.map((i) => i.id)).size).toBe(25);
    expect(shuffled.done).toBe(true);
  });

  it("reads no more than the cap", async () => {
    const fetchPage = vi.fn(async (cursor: string | null) => ({
      items: photos(1000, `c${cursor ?? 0}-`),
      next_cursor: String(Number(cursor ?? 0) + 1),
    }));
    const r = await pagedSource(fetchPage, "s").open("shuffle", "k", 0).next();
    expect(r.items.length).toBeLessThanOrEqual(MAX_LOCAL);
    expect(fetchPage.mock.calls.length).toBeLessThanOrEqual(MAX_LOCAL / 1000 + 1);
  });

  it("a read that fails halfway is resumed, not restarted", async () => {
    const { fetchPage } = pages(25, 10);
    let failOnce = true;
    const flaky = vi.fn(async (cursor: string | null) => {
      if (cursor === "10" && failOnce) {
        failOnce = false;
        throw new Error("down");
      }
      return fetchPage(cursor);
    });
    const feed = pagedSource(flaky, "s").open("oldest", "", 0);
    await expect(feed.next()).rejects.toThrow("down");
    expect((await feed.next()).items).toHaveLength(25);
    expect(fetchPage).toHaveBeenCalledTimes(3);
  });
});

describe("itemsSource and arrange", () => {
  it("starts the first pass at the chosen photo, the later ones at the beginning", async () => {
    const src = itemsSource(photos(5), "Photos", "p3");
    expect((await src.open("listed", "", 0).next()).items.map((i) => i.id)).toEqual(["p3", "p4", "p5"]);
    expect((await src.open("listed", "", 1).next()).items.map((i) => i.id)).toEqual(["p1", "p2", "p3", "p4", "p5"]);
  });

  it("a shuffle shows the chosen photo first and then every other one once", () => {
    const out = arrange(photos(10), "shuffle", "z", "p7");
    expect(out[0]!.id).toBe("p7");
    expect(new Set(out.map((i) => i.id)).size).toBe(10);
  });

  it("oldest first from a photo goes forward in time from it", () => {
    const out = arrange(photos(5), "oldest", "", "p3");
    expect(out.map((i) => i.id)).toEqual(["p3", "p2", "p1"]);
  });

  it("an unknown start photo changes nothing", () => {
    expect(arrange(photos(3), "listed", "", "nope").map((i) => i.id)).toEqual(["p1", "p2", "p3"]);
  });
});

describe("searchSource", () => {
  it("a filter-only search is a scope the server pages, without the words and chips", async () => {
    const page = vi.spyOn(api, "slideshowPage").mockResolvedValue({ items: [], next_cursor: null, total: 0 });
    await searchSource({ tags: "trip", favorite: true }, "r").open("listed", "", 0).next();
    expect(page.mock.calls[0]![0]).toEqual({ search: { tags: "trip", favorite: true } });
  });

  it("a search with words is read through its own pages", async () => {
    const search = vi.spyOn(api, "search").mockResolvedValue({ items: photos(2) as never, next_cursor: null, ranked: true });
    const page = vi.spyOn(api, "slideshowPage");
    const r = await searchSource({ q: "dogs" }, "r").open("listed", "", 0).next();
    expect(r.items).toHaveLength(2);
    expect(page).not.toHaveBeenCalled();
    expect(search).toHaveBeenCalledWith({ q: "dogs" }, { cursor: null, limit: 100 });
  });
});

describe("PreviewCache", () => {
  const info = (id: string) => ({ id, preview_url: `https://s3.test/${id}`, width: 4, height: 3, caption: null });

  it("fetches the wanted photo and the ones after it in one request, and reuses them", async () => {
    const fetchPreviews = vi.fn(async (ids: readonly string[]) => ({ items: ids.map(info) }));
    const cache = new PreviewCache(fetchPreviews);
    expect((await cache.get("a", ["b", "c"]))?.preview_url).toBe("https://s3.test/a");
    expect(fetchPreviews).toHaveBeenCalledTimes(1);
    expect(fetchPreviews).toHaveBeenCalledWith(["a", "b", "c"]);
    await cache.get("b", ["c", "d"]);
    expect(fetchPreviews).toHaveBeenCalledTimes(1);
    await cache.get("d", []);
    expect(fetchPreviews).toHaveBeenLastCalledWith(["d"]);
  });

  it("asks again once an address is old, and when told it failed", async () => {
    let now = 0;
    const fetchPreviews = vi.fn(async (ids: readonly string[]) => ({ items: ids.map(info) }));
    const cache = new PreviewCache(fetchPreviews, () => now);
    await cache.get("a");
    now += 2 * 60_000;
    await cache.get("a");
    expect(fetchPreviews).toHaveBeenCalledTimes(1);
    now += 2 * 60_000;
    await cache.get("a");
    expect(fetchPreviews).toHaveBeenCalledTimes(2);
    cache.forget("a");
    await cache.get("a");
    expect(fetchPreviews).toHaveBeenCalledTimes(3);
  });

  it("a photo the server left out is null, and is not asked for again at once", async () => {
    const fetchPreviews = vi.fn(async () => ({ items: [] }));
    const cache = new PreviewCache(fetchPreviews);
    expect(await cache.get("gone")).toBeNull();
    expect(await cache.get("gone")).toBeNull();
    expect(fetchPreviews).toHaveBeenCalledTimes(1);
  });

  it("joins a request that is already out, and never asks for more than 24", async () => {
    let release: () => void = () => undefined;
    const fetchPreviews = vi.fn(async (ids: readonly string[]) => {
      await new Promise<void>((r) => (release = r));
      return { items: ids.map(info) };
    });
    const cache = new PreviewCache(fetchPreviews);
    const many = Array.from({ length: 40 }, (_, i) => `x${i}`);
    const a = cache.get("a", many);
    const b = cache.get("x1");
    await Promise.resolve();
    release();
    await Promise.all([a, b]);
    expect(fetchPreviews).toHaveBeenCalledTimes(1);
    expect(fetchPreviews.mock.calls[0]![0]).toHaveLength(24);
  });

  it("holds a bounded number of addresses", async () => {
    const fetchPreviews = vi.fn(async (ids: readonly string[]) => ({ items: ids.map(info) }));
    const cache = new PreviewCache(fetchPreviews);
    for (let i = 0; i < 300; i++) await cache.get(`p${i}`);
    await cache.get("p0");
    expect(fetchPreviews).toHaveBeenCalledTimes(301);
  });
});

describe("settings", () => {
  afterEach(() => localStorage.clear());

  it("falls back to the defaults field by field", () => {
    expect(parseSettings(null)).toEqual(DEFAULT_SETTINGS);
    expect(parseSettings({ duration: 7, order: "sideways", transition: "none", loop: "yes" })).toEqual({ ...DEFAULT_SETTINGS, transition: "none" });
    expect(parseSettings({ duration: 20, order: "shuffle", transition: "crossfade", loop: false })).toEqual({
      duration: 20,
      order: "shuffle",
      transition: "crossfade",
      loop: false,
    });
  });

  it("remembers per browser, and survives storage that is missing or broken", () => {
    saveSettings({ duration: 10, order: "oldest", transition: "none", loop: false });
    expect(loadSettings()).toEqual({ duration: 10, order: "oldest", transition: "none", loop: false });
    localStorage.setItem(SETTINGS_KEY, "{not json");
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(loadSettings()).toEqual(DEFAULT_SETTINGS);
    expect(() => saveSettings(DEFAULT_SETTINGS)).not.toThrow();
  });
});
