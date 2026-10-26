// The Search page at phone and desktop widths: the always-visible field, the suggestions before anything is typed, recent
// searches, results with their count and selection, the Filters sheet and its chips, what the server read out of a query,
// weaker matches, indexing notes, bursts, "more like this", the empty and error states, and Ctrl+K. Nothing may stick out
// sideways. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import type { FakeAsset } from "./fakeLibrary";
import { json, LONG_NAME, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG_CAMERA = "Hasselblad X2D 100C with the XCD 4/45P lens and the optional extension tube set";
const LONG_PLACE = "Llanfairpwllgwyngyllgogerychwyrndrobwllllantysiliogogogoch";
const id = (n: number) => `6f1c2b0e-0000-4000-8000-${String(n).padStart(12, "0")}`;
const FACETS = {
  total: 52000,
  favorites: 412,
  with_location: 30011,
  with_text: 840,
  ratings: [{ value: "5", count: 120 }],
  types: [
    { value: "jpeg", count: 41000 },
    { value: "heic", count: 9000 },
    { value: "raw", count: 6000 },
  ],
  cameras: [
    { value: LONG_CAMERA, count: 5230 },
    { value: "Canon EOS R5", count: 30100 },
  ],
  lenses: [],
  places: [
    { value: "United Kingdom", kind: "country", count: 3000 },
    { value: LONG_PLACE, kind: "city", count: 12 },
    { value: "Lisbon", kind: "city", count: 4100 },
    { value: "Portugal", kind: "country", count: 4500 },
  ],
  years: [
    { value: "2026", count: 3000 },
    { value: "2024", count: 9000 },
    { value: "2023", count: 12000 },
    { value: "2019", count: 7000 },
  ],
};
const person = (n: number, name: string, photos: number, hidden = false) => ({
  id: id(n),
  name,
  hidden,
  faces: photos,
  photos,
  cover: { face_id: `f${n}`, asset_id: `a${n}`, box: [0.4, 0.3, 0.2, 0.2], preview_url: `/fixture-thumbs/${n}.svg` },
});
const PEOPLE = [
  person(1, "A really quite long name with_an_unbroken_run_of_characters_that_could_overflow_" + "x".repeat(10), 900),
  person(2, "Person B", 400),
  ...Array.from({ length: 8 }, (_, i) => person(3 + i, `Person ${String.fromCharCode(67 + i)}`, 50 - i)),
  person(20, "Hidden person", 5, true),
];

const IDLE = { active: false, queued: 0, running: 0, failed: 0, by_kind: [], total_photos: 30, without_thumbnail: 0, failures: [] };
const INDEXING = { ...IDLE, total_photos: 52000, embedding: { indexed: 12345, total: 52000, queued: 39654, running: 1, failed: 0 } };
const REINDEXING = {
  ...IDLE,
  total_photos: 52000,
  embedding: { indexed: 20800, total: 52000, queued: 31199, running: 1, failed: 0, model: "siglip2-b16", percent: 40, reindexing: true, switch_at_percent: 95, serving_model: "clip-b32" },
};
// What the server reads out of a query: a long place and a date, so the chips are as wide as they get.
const PARSED = {
  text: "dog",
  chips: [
    { kind: "date", label: "Summer 2022", matched: "in summer 2022", from: "2022-06-01", to: "2022-08-31" },
    { kind: "place", label: LONG_PLACE, matched: "in " + LONG_PLACE, value: LONG_PLACE, place_kind: "city" },
  ],
};

for (const { width, scheme } of RUNS) {
  test.describe(`search ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("suggestions, results, filters, chips and states fit", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("search", width, scheme), problems);
      const desktop = width >= 768;
      let processing: typeof IDLE | typeof INDEXING | typeof REINDEXING = IDLE;
      let assets: FakeAsset[] = [];
      const hits = (n: number, weak = 0) =>
        assets.slice(0, n + weak).map((a, i) => ({
          ...a,
          original_name: i === 1 ? LONG_NAME : a.original_name,
          score: 0.31 - i * 0.004,
          probability: 0.1,
          tier: i < n ? "strong" : "weak",
          rating: 0,
          archived: false,
          hidden: false,
          edited: false,
        }));
      const burstHits = () =>
        hits(10).map((h, i) =>
          i === 1 || i === 4
            ? { ...h, burst: { size: i === 1 ? 4 : 12, members: assets.slice(12, 12 + (i === 1 ? 3 : 11)).map((a) => ({ ...a, score: 0.2 })) } }
            : h,
        );
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          const ok = (body: unknown, status = 200) => {
            void route.fulfill(json(body, status));
            return true;
          };
          if (p === "/assets/people/status") return ok({ enabled: true });
          if (p === "/assets/people" && method === "GET") return ok({ items: PEOPLE });
          if (p === "/assets/search/facets") return ok(FACETS);
          if (p === "/assets/tags" && method === "GET") return ok({ items: [] });
          if (p === "/assets/search" && method === "GET") {
            const q = url.searchParams.get("q") ?? "";
            if (q.includes("burst"))
              return ok({ items: burstHits(), next_cursor: null, ranked: true, total: null, strong_count: 10, weaker: null, parsed: { text: q, chips: [] } });
            if (q.includes("unavailable")) return ok({ error: "search is not available: the model files are not installed" }, 503);
            const weaker = url.searchParams.get("weaker") === "true";
            // "nothing": no strong match and some weaker ones; "empty": nothing at all; "weak": a few strong and many weaker.
            const items = q.includes("empty")
              ? []
              : q.includes("nothing")
                ? weaker ? hits(0, 6) : []
                : q.includes("weak")
                  ? weaker ? hits(8, 12) : hits(8)
                  : hits(q ? 24 : 30);
            const ranked = q !== "";
            const extra = !ranked
              ? {}
              : {
                  strong_count: q.includes("nothing") ? 0 : items.length,
                  weaker: !weaker && (q.includes("nothing") || q.includes("weak")) ? { count: q.includes("weak") ? 12 : 6, cursor: q.includes("weak") ? "after-strong" : null } : null,
                  parsed: q.includes("summer") ? PARSED : { text: q, chips: [] },
                  reindexing: processing === REINDEXING ? { model: "siglip2-b16", indexed: 20800, total: 52000, percent: 40, switch_at_percent: 95 } : null,
                };
            return ok({ items, next_cursor: null, ranked, total: ranked ? null : items.length, ...extra });
          }
          // "More like this": the id ending in `dead` is not indexed yet.
          const sm = /^\/assets\/([0-9a-f-]{36})\/similar$/.exec(p);
          if (sm && method === "GET") {
            if (sm[1]!.endsWith("dead")) return ok({ error: "this photo is still being indexed: similar photos are available once it is" }, 409);
            const weaker = url.searchParams.get("weaker") === "true";
            return ok({ items: weaker ? hits(8, 12) : hits(8), next_cursor: null, ranked: true, total: null, strong_count: 8, weaker: weaker ? null : { count: 12, cursor: "after-strong" }, similar_to: sm[1] });
          }
          const um = /^\/assets\/([0-9a-f-]{36})$/.exec(p);
          if (um && method === "GET") return ok({ ...assets[3], id: um[1], albums: [], original_url: assets[3]!.thumb_url, preview_url: assets[3]!.thumb_url });
          return false;
        },
      ];
      assets = await setupApp(page, { handlers });
      // (the shared fake answers "idle" first; this later route wins)
      await page.route((u) => u.pathname === "/assets/processing", (r) => r.fulfill(json(processing)));
      const field = page.getByRole("combobox", { name: "Search photos" });

      // Before anything is typed: the field is focused and the page suggests (recent searches, examples, people, places, kinds, dates).
      await page.addInitScript(() =>
        localStorage.setItem("cyanotype.recentSearches", JSON.stringify(["dog on the beach", "red car at night", "x".repeat(90)])),
      );
      await page.goto("/search");
      await page.getByRole("heading", { level: 1, name: "Search" }).waitFor();
      await expect(field).toBeFocused();
      await page.getByRole("list", { name: "Dates" }).waitFor();
      await page.getByRole("list", { name: "Places" }).waitFor();
      await page.getByRole("list", { name: "People" }).waitFor();
      await page.getByRole("list", { name: "Kinds of photo" }).waitFor();
      await page.getByRole("list", { name: "Recent searches" }).getByRole("button").first().waitFor();
      await page.waitForTimeout(300);
      await check("intro");
      if (!desktop) {
        await page.mouse.wheel(0, 2000);
        await check("intro-scrolled");
        await page.mouse.wheel(0, -2000);
      }

      // Typing drops the recent searches that continue it down under the field.
      await field.fill("dog");
      await page.getByRole("listbox", { name: "Recent searches" }).waitFor();
      await check("typing");
      await field.fill("");

      // A tap on a suggestion searches; the people suggestion filters by that person.
      await page.getByRole("list", { name: "People" }).getByRole("link").nth(1).click();
      await page.getByRole("list", { name: "Active filters" }).getByText("Person B").waitFor();
      await page.getByRole("button", { name: /Open IMG_/ }).first().waitFor();
      await check("person-results");

      await page.goto("/search");
      await field.fill("dog on the beach");
      await field.press("Enter");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByText(/^\d+ matches$/).waitFor();
      await check("results");
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await check("results-select");
      await page.getByRole("button", { name: "Done" }).click();

      // The Filters sheet: the library's own choices, long names included.
      await page.getByRole("button", { name: /^Filters/ }).click();
      const sheet = page.getByRole("dialog", { name: "Filters" });
      await sheet.waitFor();
      await sheet.getByRole("option", { name: new RegExp(LONG_CAMERA.slice(0, 20)) }).first().waitFor({ state: "attached" });
      await page.waitForTimeout(300);
      await check("filters-sheet");
      await sheet.getByLabel("Camera", { exact: true }).selectOption(LONG_CAMERA);
      await sheet.getByLabel("Place", { exact: true }).selectOption(LONG_PLACE);
      await sheet.getByLabel("Year", { exact: true }).selectOption("2024");
      await sheet.getByRole("switch", { name: /Favorites only/ }).click();
      await sheet.getByText("RAW", { exact: true }).click();
      await sheet.getByRole("group", { name: "File type" }).scrollIntoViewIfNeeded();
      await check("filters-sheet-filled");
      await sheet.getByRole("button", { name: "Show photos" }).click();
      await sheet.waitFor({ state: "detached" });
      await page.getByRole("list", { name: "Active filters" }).getByText("2024").waitFor();
      await check("results-with-chips");

      // Taking a chip off, and all of them.
      await page.getByRole("button", { name: "Remove filter Favorites" }).click();
      await page.waitForURL((u) => !u.search.includes("favorite"));
      await page.getByRole("button", { name: "Clear filters" }).click();
      await page.waitForURL((u) => !u.search.includes("camera"));
      await check("filters-cleared");

      await field.fill("empty like it");
      await field.press("Enter");
      await page.getByRole("heading", { name: "Nothing matched" }).waitFor();
      await check("no-results");

      // Nothing matched because of the filters: they can be cleared from the empty state.
      await page.goto("/search?q=empty&favorite=true&type=raw");
      await page.getByRole("heading", { name: "Nothing matched" }).waitFor();
      await check("no-results-filters");

      // The query read as a date and a place: two chips (the place as long as a place name can be), and taking one off.
      await page.goto("/search?q=dog%20in%20summer%202022");
      await page.getByRole("list", { name: "Understood from your search" }).waitFor();
      await check("parsed-chips");
      await page.getByRole("button", { name: "Remove date Summer 2022" }).click();
      await page.waitForURL(/skip=date/);
      await check("parsed-chip-removed");

      // Strong matches with weaker ones behind them, and the weaker ones shown.
      await page.goto("/search?q=weak");
      await page.getByRole("button", { name: /Show weaker matches \(12\)/ }).waitFor();
      await check("weaker-offered");
      await page.getByRole("button", { name: /Show weaker matches/ }).click();
      await page.getByText("8 matches, 12 weaker").waitFor();
      await check("weaker-shown");
      // Nothing close, but weaker matches exist.
      await page.goto("/search?q=nothing%20close");
      await page.getByRole("heading", { name: "No close matches" }).waitFor();
      await check("no-close-matches");
      await page.getByRole("button", { name: /Show weaker matches \(6\)/ }).click();
      await page.getByText("0 matches, 6 weaker").waitFor();
      await check("no-close-matches-weaker-shown");

      // An older model answers while the improved one indexes the library: said once, with progress.
      processing = REINDEXING;
      await page.goto("/search?q=dog");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByText(/Re-indexing with the improved search model: 40%/).first().waitFor();
      await check("reindexing");
      await page.goto("/search");
      await page.getByRole("heading", { name: "Search your photos" }).waitFor();
      await page.getByText(/Re-indexing with the improved search model: 40%/).waitFor();
      await check("reindexing-intro");

      processing = INDEXING;
      await page.goto("/search?q=dog");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByText(/Still indexing 39,655 photos/).first().waitFor();
      await check("indexing");
      await page.goto("/search?q=nothing");
      await page.getByText(/still being indexed cannot match/).waitFor();
      await check("indexing-no-results");
      processing = IDLE;

      await page.goto("/search?q=unavailable");
      await page.getByRole("heading", { name: "Search isn’t available" }).waitFor();
      await check("unavailable");

      // Bursts: collapsed with their badges, one opened inline, and selecting a collapsed burst (the bar says what it includes).
      await page.goto("/search?q=burst");
      await page.getByRole("button", { name: "Show 3 more photos from this burst" }).waitFor();
      await check("bursts");
      await page.getByRole("button", { name: "Show 3 more photos from this burst" }).click();
      await page.getByRole("button", { name: "Hide the other 3 photos of this burst" }).waitFor();
      await check("burst-open");
      await page.getByRole("button", { name: "Hide the other 3 photos of this burst" }).click();
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /^Select .* and 3 more from its burst/ }).click();
      await page.getByText("4 selected").waitFor();
      await check("burst-selected");
      await page.getByRole("button", { name: "Done" }).click();

      // "More like this": the source chip with its thumbnail, the filters beside it, weaker matches, and a photo not indexed yet.
      await page.goto("/search?similar=6f1c2b0e-0000-4000-8000-0000000000a3&favorite=true");
      await page.getByText("8 similar photos").waitFor();
      await check("similar");
      await page.getByRole("button", { name: /Show weaker matches \(12\)/ }).click();
      await page.getByText("8 similar photos, 12 weaker").waitFor();
      await check("similar-weaker-shown");
      await page.goto("/search?similar=6f1c2b0e-0000-4000-8000-00000000dead");
      await page.getByRole("heading", { name: "This photo is still being indexed" }).waitFor();
      await check("similar-not-indexed");

      // Ctrl+K from another page lands on Search with the field focused (a keyboard on a phone is rare, but harmless).
      await page.goto("/albums");
      await page.getByRole("heading", { level: 1, name: "Albums" }).waitFor();
      await page.keyboard.press("Control+k");
      await page.waitForURL(/\/search$/);
      await expect(field).toBeFocused();

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
