// The home page (Collections), a trip, On this day and Places at phone and desktop widths: every section in place,
// nothing sticking out sideways, the rows scrolling on their own, the map under its frosted header with the photos in a
// sheet (phone) or a side panel (desktop). Set SHOT_DIR to save the screenshots.
import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { makeAssets, type FakeAsset } from "./fakeLibrary";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const WORLD_TILES = readFileSync(new URL("./fixtures/world-z2.pmtiles", import.meta.url));
const thumb = (n: number) => `/fixture-thumbs/${n}.svg`;
const LONG = "Summer road trip along the entire Pacific coast highway with the whole extended family, 2019 to 2024";

const item = (a: FakeAsset) => ({ ...a });
const trip = (assets: FakeAsset[], n: number, title: string, over: Record<string, unknown> = {}) => {
  const month = [9, 8, 7, 6][n - 1]!;
  const from = `2026-0${month}-1${n}`;
  const to = `2026-0${month}-1${n + 3}`;
  return {
    id: `${from}_${to}`,
    from,
    to,
    ongoing: false,
    title,
    date_label: `${["September", "August", "July", "June"][n - 1]} 2026`,
    place: { name: title, level: "city", city: title, country: "Somewhere", country_code: "XX" },
    photos: 40 * n + 3,
    cover_id: assets[n]!.id,
    cover: item(assets[n]!),
    ...over,
  };
};

function home(assets: FakeAsset[], opts: { delay?: number } = {}): Handler {
  const trips = [
    trip(assets, 1, "Lisbon", { ongoing: true, to: new Date().toISOString().slice(0, 10), id: "2026-10-20_" + new Date().toISOString().slice(0, 10), from: "2026-10-20", date_label: "October 2026" }),
    trip(assets, 2, "Copenhagen"),
    trip(assets, 3, "A rather long place name for a trip that nobody would ever really call this"),
    trip(assets, 4, "Kyoto"),
  ];
  const albums = Array.from({ length: 9 }, (_, i) => ({
    id: `al${i}`,
    title: i === 1 ? LONG : ["Family", "Garden", "Beach days", "Birthday", "Hiking", "Food", "Pets", "Road trip", "Winter"][i],
    description: null,
    photo_count: i === 3 ? 1 : 12 + i * 37,
    cover: i === 8 ? null : { id: assets[i + 5]!.id, thumbhash: null, thumb_url: thumb(i + 5) },
    first_at: "2025-06-10T12:00:00Z",
    last_at: "2026-01-20T12:00:00Z",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-02T00:00:00Z",
  }));
  const years = [1, 3].map((y, i) => ({ year: 2026 - y, years_ago: y, count: 6 + i * 1228, items: assets.slice(10 + i * 6, 16 + i * 6).map(item) }));
  const people = ["Alpha", null, "Bravo with a long name", "Charlie", null, "Delta", "Echo", "Foxtrot"].map((name, i) => ({
    id: `pe${i}`,
    name,
    hidden: false,
    faces: 9,
    photos: 40 - i,
    cover: { face_id: `f${i}`, asset_id: assets[i]!.id, box: [0.3, 0.3, 0.3, 0.3], preview_url: thumb(i + 1) },
  }));
  const shares = [
    { id: "s1", kind: "album", album: { id: "al0", title: "Family" }, title: null, token: "t1", has_password: false, allow_download: true, include_originals: false, expires_at: null, expired: false, item_count: 48, view_count: 3, last_viewed_at: null, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
    { id: "s2", kind: "selection", album: null, title: LONG, token: "t2", has_password: true, allow_download: false, include_originals: false, expires_at: null, expired: false, item_count: 1, view_count: 1, last_viewed_at: null, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
  ];
  return async (route, url, method) => {
    const p = url.pathname;
    if (method !== "GET") return false;
    const answer = async (body: unknown) => {
      if (opts.delay) await new Promise((r) => setTimeout(r, opts.delay));
      await route.fulfill(json(body));
      return true;
    };
    if (p === "/assets/memories") return answer({ enabled: true, day: "10-26", years });
    if (p === "/assets/trips") return answer({ enabled: true, trips, next_before: null });
    if (p === "/assets/albums") return answer({ items: albums });
    if (p === "/assets/people/status") return answer({ enabled: true });
    if (p === "/assets/people") return answer({ items: people });
    if (p === "/assets/shares") return answer({ items: shares });
    return false;
  };
}

/** The tile archive and map markers of the Places page. */
const mapHandler: Handler = async (route, url, method) => {
  const p = url.pathname;
  if (p === "/assets/tiles" && method === "GET") return route.fulfill(json({ world: true, detail: false })).then(() => true);
  if (p === "/assets/tiles/world.pmtiles") {
    const r = /bytes=(\d+)-(\d+)/.exec(route.request().headers()["range"] ?? "");
    if (!r) return route.fulfill({ status: 400, body: "" }).then(() => true);
    const first = Number(r[1]);
    const last = Math.min(Number(r[2]), WORLD_TILES.length - 1);
    await route.fulfill({
      status: 206,
      contentType: "application/octet-stream",
      headers: { "content-range": `bytes ${first}-${last}/${WORLD_TILES.length}`, "accept-ranges": "bytes", etag: '"fixture"' },
      body: WORLD_TILES.subarray(first, last + 1),
    });
    return true;
  }
  if (p === "/assets/map" && method === "GET") {
    const m = (id: string, lat: number, lon: number, count: number, n: number) => ({
      id, lat, lon, count, bounds: count > 1 ? [lon - 0.4, lat - 0.3, lon + 0.4, lat + 0.3] : null, thumbhash: null, thumb_url: count === 1 ? thumb(n) : null,
    });
    const clusters = [m("a1", 30.04, 31.24, 1, 1), m("a2", 51.5, -0.12, 7, 2), m("a3", 40.7, -74.0, 128, 3), m("a4", 35.68, 139.65, 12400, 4), m("a5", -33.86, 151.2, 1, 5)];
    await route.fulfill(json({ clusters, total: 12537, truncated: false }));
    return true;
  }
  return false;
};

async function scrollEnd(page: Page) {
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  await page.waitForTimeout(150);
}

for (const { width, scheme } of RUNS) {
  test.describe(`collections ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });
    const desktop = width >= 768;

    test("the home page: sections, rows, the list of ways in, and a trip", async ({ page }) => {
      test.setTimeout(90_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("collections", width, scheme), problems);
      await setupApp(page, { count: 60, handlers: [home(makeAssets(60, { yearsBack: 1 }))] });

      await page.goto("/");
      await expect(page.getByRole("heading", { level: 1, name: "Collections" })).toBeVisible();
      for (const name of ["Memories", "Albums", "Recent places", "People", "Shared links", "Recently added"])
        await expect(page.getByRole("region", { name })).toBeVisible();
      await page.waitForLoadState("networkidle");
      await check("home");
      await scrollEnd(page);
      await check("home-bottom");
      if (desktop) {
        await expect(page.getByRole("region", { name: "Utilities" })).toBeHidden();
      } else {
        const utilities = page.getByRole("region", { name: "Utilities" });
        await expect(utilities.getByRole("link")).toHaveText(["Archive", "Hidden", "Duplicates", "Recently deleted"]);
        await expect(page.getByRole("region", { name: "Library" }).getByRole("link", { name: "Places" })).toHaveAttribute("href", "/map");
      }

      // A row scrolls on its own, and with a mouse shows arrows at its end.
      const row = page.getByRole("list", { name: "Recent places" });
      await page.evaluate(() => window.scrollTo(0, 0));
      expect(await row.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
      if (desktop) {
        await row.hover();
        const next = page.getByRole("button", { name: "Scroll Recent places forward" });
        await expect(next).toBeVisible();
        await check("home-arrows");
        await next.click();
        await expect.poll(() => row.evaluate((el) => el.scrollLeft)).toBeGreaterThan(50);
        await expect(page.getByRole("button", { name: "Scroll Recent places back" })).toBeVisible();
      } else {
        await expect(page.getByRole("button", { name: /^Scroll / })).toHaveCount(0);
        expect(await row.evaluate((el) => getComputedStyle(el).scrollSnapType)).toContain("x");
      }

      // A trip opens its photos, and back goes home.
      await page.getByRole("region", { name: "Recent places" }).getByRole("link", { name: /^Copenhagen/ }).click();
      await expect(page.getByRole("heading", { level: 1, name: "Copenhagen" })).toBeVisible();
      await expect(page).toHaveURL(/\/trips\/2026-08-12_2026-08-15$/);
      await page.getByRole("button", { name: /^Open IMG_/ }).first().waitFor();
      await expect(page.getByText(/Aug 12 – 15, 2026 · 83 photos/)).toBeVisible();
      await check("trip");
      await page.getByRole("button", { name: /^Open IMG_/ }).first().click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await check("trip-lightbox");
      await page.keyboard.press("Escape");
      await page.getByRole("link", { name: "Collections" }).first().click();
      await expect(page).toHaveURL(/\/$/);

      // A trip card is still a link when opened from the shell's own list.
      await page.getByRole("region", { name: "Recent places" }).getByRole("link", { name: "Open the map of your places" }).click();
      await expect(page).toHaveURL(/\/map$/);
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("rows show skeletons while their sections load", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("collections-states", width, scheme), problems);
      await setupApp(page, { count: 60, handlers: [home(makeAssets(60, { yearsBack: 1 }), { delay: 2500 })] });
      await page.goto("/");
      await expect(page.locator("[data-scroll-row][aria-hidden=true]").first()).toBeVisible();
      await check("loading");
      await expect(page.getByRole("region", { name: "Albums" }).getByRole("list", { name: "Albums" })).toBeVisible();
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("an empty library invites an upload", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("collections-empty", width, scheme), problems);
      await setupApp(page, { count: 1 });
      // The library's own list is answered by the fake before any handler: override it.
      await page.route(
        (u) => u.pathname === "/assets",
        (r) => r.fulfill(json({ items: [], next_cursor: null, next_before: null })),
      );
      await page.goto("/");
      await expect(page.getByRole("heading", { level: 2, name: "Start your library" })).toBeVisible();
      await check("empty");
      await scrollEnd(page);
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("On this day: the years, selecting for Not this one, and the menu", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("memories", width, scheme), problems);
      const assets = makeAssets(60, { yearsBack: 1 });
      let enabled = true;
      let prefs: unknown = null;
      await setupApp(page, {
        count: 60,
        handlers: [
          async (route, url, method) => {
            if (url.pathname === "/assets/prefs" && method === "PUT") {
              prefs = route.request().postDataJSON();
              enabled = (prefs as { memories_enabled: boolean }).memories_enabled;
              await route.fulfill(json({ memories_enabled: enabled, library_sort: "taken", stacks_collapsed: true }));
              return true;
            }
            if (url.pathname === "/assets/memories" && method === "GET") {
              const years = [1, 3, 12].map((y, i) => ({ year: 2026 - y, years_ago: y, count: 6 + i, items: assets.slice(i * 8, i * 8 + 6 + i).map(item) }));
              await route.fulfill(json({ enabled, day: "10-26", years }));
              return true;
            }
            return false;
          },
        ],
      });
      await page.goto("/memories");
      await expect(page.getByRole("heading", { level: 1, name: "On this day" })).toBeVisible();
      await expect(page.getByRole("heading", { level: 2, name: /3 years ago/ })).toBeVisible();
      await page.getByRole("button", { name: /^Open IMG_/ }).first().waitFor();
      await check("page");
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /^Select IMG_/ }).first().click();
      await expect(page.getByRole("button", { name: "Not this one" })).toBeEnabled();
      await check("select");
      await page.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Hide from Collections" }).click();
      await expect.poll(() => prefs).toEqual({ memories_enabled: false });
      await expect(page.getByRole("status").filter({ hasText: "hidden from Collections" })).toBeVisible();
      await check("hidden");
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("Places: the map under its header, the photos in a sheet or a side panel", async ({ page }) => {
      test.setTimeout(90_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("places", width, scheme), problems);
      await setupApp(page, { count: 60, handlers: [mapHandler] });
      await page.goto("/map");
      await expect(page.getByRole("heading", { level: 1, name: "Places" })).toBeVisible();
      await page.locator("[data-marker]").first().waitFor({ timeout: 20_000 });
      await page.waitForTimeout(1500); // tiles and labels draw
      await check("map");
      if (!desktop) {
        // Folded: the count and the handle. Unfolded: half the map, and the photos.
        await expect(page.getByText(/photos in view/)).toBeVisible();
        await page.getByRole("button", { name: "Show photos in view" }).click();
        await expect(page.getByRole("button", { name: "Fold photos in view" })).toBeVisible();
        await page.waitForTimeout(500);
        await check("map-sheet");
      }
      await page.getByRole("button", { name: /^Open IMG_/ }).first().click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await check("map-lightbox");
      await page.keyboard.press("Escape");
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
