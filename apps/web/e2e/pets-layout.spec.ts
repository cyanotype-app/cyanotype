// Pets at phone and desktop widths: the Collections row, the Pets page (the switch, the timeline, selection, the menu), the page while the
// library is still being looked through and when nothing was found, the chip a search makes of "dogs", the filter in the filters sheet, the smart
// album rule, and everything gone while Pets is off. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { pageOf, timelineOf, type FakeAsset } from "./fakeLibrary";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const cover = (n: number) => ({ asset_id: `a${n}`, box: [0.35, 0.3, 0.3, 0.3], preview_url: `/fixture-thumbs/${n}.svg` });
const SUMMARY = {
  any: 9,
  kinds: [
    { kind: "dog", photos: 6, cover: cover(1) },
    { kind: "cat", photos: 3, cover: cover(2) },
  ],
  scanned: 30,
  total: 30,
};
const SCANNING = { any: 0, kinds: [{ kind: "dog", photos: 0, cover: null }, { kind: "cat", photos: 0, cover: null }], scanned: 1234, total: 5000 };
const NONE = { ...SCANNING, scanned: 30, total: 30 };

/** The Pets routes: `summary` is what GET /assets/pets answers, `mine` the photos of every kind (dogs are the first six). */
function pets(opts: { enabled: boolean; summary?: unknown; mine: () => FakeAsset[]; calls?: { path: string; body: unknown }[] }): Handler {
  return (route, url, method) => {
    const p = url.pathname;
    if (p === "/assets/pets/status") {
      void route.fulfill(json({ enabled: opts.enabled }));
      return true;
    }
    if (!opts.enabled) return false;
    if (p === "/assets/pets") {
      void route.fulfill(json(opts.summary ?? SUMMARY));
      return true;
    }
    const kind = url.searchParams.get("kind");
    const all = opts.mine();
    const list = kind === "dog" ? all.slice(0, 6) : kind === "cat" ? all.slice(6, 9) : all;
    if (p === "/assets/pets/timeline") {
      void route.fulfill(json(timelineOf(list, false)));
      return true;
    }
    if (p === "/assets/pets/photos") {
      const page = pageOf(list, url.searchParams);
      void route.fulfill(json({ ...page, items: page.items.map((a) => ({ ...a, rating: 0, archived: false, hidden: false, edited: false })) }));
      return true;
    }
    if (p === "/assets/search/facets") {
      void route.fulfill(
        json({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [], pets: [{ value: "any", count: 9 }, { value: "dog", count: 6 }, { value: "cat", count: 3 }] }),
      );
      return true;
    }
    if (method === "POST" && p === "/assets/albums") {
      opts.calls?.push({ path: p, body: route.request().postDataJSON() });
      void route.fulfill(json({ id: "al1", title: "Pets", description: null, photo_count: 0, cover: null, first_at: null, last_at: null, created_at: "", updated_at: "" }, 201));
      return true;
    }
    if (method === "POST" && p === "/assets/albums/al1/assets") {
      opts.calls?.push({ path: p, body: route.request().postDataJSON() });
      void route.fulfill(json({ count: 9 }));
      return true;
    }
    return false;
  };
}

for (const { width, scheme } of RUNS) {
  test.describe(`pets ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("Collections, the Pets page, selection, the menu, and the search chip without overflow", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("pets", width, scheme), problems);
      const calls: { path: string; body: unknown }[] = [];
      let mine: FakeAsset[] = [];
      const assets = await setupApp(page, { handlers: [pets({ enabled: true, mine: () => mine, calls })] });
      mine = assets.slice(0, 9);
      await page.route("**/assets/search?*", (route) => {
        const u = new URL(route.request().url());
        const items = mine;
        void route.fulfill(
          json({
            items: items.map((a) => ({ ...a, score: null, rating: 0, archived: false, hidden: false, edited: false })),
            next_cursor: null,
            ranked: false,
            total: items.length,
            parsed: u.searchParams.get("q") ? { text: "on the sofa", chips: [{ kind: "pets", label: "Dogs", matched: "dogs", pets: "dog" }] } : null,
          }),
        );
      });

      // Collections: a row of the kinds, each with its cover cropped round.
      await page.goto("/");
      const row = page.getByRole("region", { name: "Pets", exact: true });
      await row.getByRole("link", { name: "Dogs, 6 photos" }).waitFor();
      await row.getByRole("link", { name: "Cats, 3 photos" }).scrollIntoViewIfNeeded();
      await check("collections");
      if (width === 1280) await expect(page.getByRole("navigation", { name: "Library" }).getByRole("link", { name: "Pets" })).toHaveAttribute("href", "/pets");

      // The page: the switch and the timeline of every pet.
      await row.getByRole("link", { name: "See all" }).click();
      await page.getByRole("heading", { level: 1, name: "Pets" }).waitFor();
      const tabs = page.getByRole("radiogroup", { name: "Show" });
      await expect(tabs.getByRole("radio")).toHaveText(["All", "Dogs", "Cats"]);
      await page.getByRole("button", { name: /Open IMG_00000/ }).waitFor();
      await check("page");

      // One kind.
      await tabs.getByRole("radio", { name: "Cats" }).click();
      await page.getByRole("button", { name: /Open IMG_00006/ }).waitFor();
      await expect(page.getByRole("button", { name: /Open IMG_00000/ })).toHaveCount(0);
      expect(page.url()).toContain("kind=cat");
      await check("cats");
      await tabs.getByRole("radio", { name: "All" }).click();
      await page.getByRole("button", { name: /Open IMG_00000/ }).waitFor();

      // The menu.
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Save as album" }).waitFor();
      await check("menu");
      await page.keyboard.press("Escape");

      // Selecting the whole view and saving it as an album goes by the scope.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: "Select all" }).click();
      await page.getByText("9 selected").waitFor();
      await check("select-all");
      await page.getByRole("toolbar", { name: "Selection" }).getByRole("button", { name: "Save as album" }).click();
      const save = page.getByRole("dialog", { name: "Save 9 photos as an album" });
      await save.waitFor();
      await save.getByLabel("Album title").fill("Pets");
      await check("save-album");
      await save.getByRole("button", { name: "Create album" }).click();
      await page.getByText(/^Created Pets with 9 photos/).waitFor();
      expect(calls.at(-1)).toEqual({ path: "/assets/albums/al1/assets", body: { scope: { pets: "any" }, except: [] } });
      await check("saved");
      await page.getByRole("button", { name: "Done" }).click();

      // The words of a search become a removable chip.
      await page.goto("/search?q=dogs%20on%20the%20sofa");
      const chips = page.getByRole("list", { name: "Understood from your search" });
      await chips.getByText("Dogs").waitFor();
      await check("search-chip");
      await chips.getByRole("button", { name: "Remove pets Dogs" }).click();
      await page.waitForURL(/skip=pets/);

      // The filter sheet.
      await page.goto("/search?pets=cat");
      await page.getByRole("list", { name: "Active filters" }).getByText("Cats").waitFor();
      await check("search-filter-chip");
      await page.getByRole("button", { name: /Filters/ }).click();
      const dialog = page.getByRole("dialog", { name: "Filters" });
      const group = dialog.getByRole("group", { name: "Pets" });
      await group.getByRole("radio", { name: "Dogs (6)" }).waitFor();
      await expect(group.getByRole("radio", { name: "Cats (3)" })).toBeChecked();
      await group.scrollIntoViewIfNeeded();
      await check("filter-sheet");
      await page.keyboard.press("Escape");

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("while the library is being looked through, and when nothing was found", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("pets-empty", width, scheme), problems);
      let summary: unknown = SCANNING;
      await setupApp(page, { handlers: [(route, url, method) => pets({ enabled: true, summary, mine: () => [] })(route, url, method)] });
      await page.goto("/pets");
      await page.getByRole("heading", { name: "Looking for dogs and cats" }).waitFor();
      await page.getByText("Looked at 1,234 of 5,000 photos").waitFor();
      await check("scanning");
      summary = NONE;
      await page.goto("/pets");
      await page.getByRole("heading", { name: "No pets found" }).waitFor();
      await check("none");
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("is nowhere while Pets is off", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("pets-off", width, scheme), problems);
      await setupApp(page, { handlers: [pets({ enabled: false, mine: () => [] })] });
      const status = page.waitForResponse((r) => r.url().endsWith("/assets/pets/status"));
      await page.goto("/");
      await page.getByRole("heading", { level: 1, name: "Collections" }).waitFor();
      await status;
      await expect(page.getByRole("region", { name: "Pets", exact: true })).toHaveCount(0);
      await expect(page.getByRole("link", { name: "Pets" })).toHaveCount(0);
      await check("collections");
      await page.goto("/pets");
      await page.getByRole("heading", { name: "Page not found" }).waitFor();
      await page.goto("/search?pets=dog");
      await page.getByRole("button", { name: /Filters/ }).click();
      await page.getByRole("dialog", { name: "Filters" }).waitFor();
      await expect(page.getByRole("group", { name: "Pets" })).toHaveCount(0);
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
