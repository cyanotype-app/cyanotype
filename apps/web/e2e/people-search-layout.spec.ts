// The People parts of search and smart albums at phone and desktop widths: the filter sheet's People group with long names and a long
// list, the chip of the people filter, the chip read out of a query, "Save as album" over the results, and the People rule of the smart
// album editor. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import type { FakeAsset } from "./fakeLibrary";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG = "A really quite long name with_an_unbroken_run_of_characters_that_could_overflow_" + "x".repeat(10);
const id = (n: number) => `6f1c2b0e-0000-4000-8000-${String(n).padStart(12, "0")}`;
const person = (n: number, name: string | null, photos: number, over: Record<string, unknown> = {}) => ({
  id: id(n),
  name,
  hidden: false,
  faces: photos,
  photos,
  cover: null,
  ...over,
});
// Twelve named people (the picker shows eight and says there are more), one hidden and one unnamed (never offered).
const PEOPLE = [
  person(1, LONG, 12345),
  person(2, "Person B", 40),
  ...Array.from({ length: 10 }, (_, i) => person(3 + i, `Person ${String.fromCharCode(67 + i)}`, 5)),
  person(13, "Person Hidden", 3, { hidden: true }),
  person(14, null, 9),
];
const FACETS = { total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [] };
const RULES = { match: "all", archived: "include", rules: [{ field: "people", ids: [id(1), id(2)] }] };

for (const { width, scheme } of RUNS) {
  test.describe(`people in search and smart albums ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("filter sheet, chips, save as album and the smart rule without overflow", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("people-search", width, scheme), problems);
      const calls: { method: string; path: string; body: unknown }[] = [];
      const searches: string[] = [];
      let assets: FakeAsset[] = [];
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          if (p === "/assets/people/status") {
            void route.fulfill(json({ enabled: true }));
            return true;
          }
          if (p === "/assets/people" && method === "GET") {
            void route.fulfill(json({ items: PEOPLE }));
            return true;
          }
          if (p === "/assets/search") {
            searches.push(url.search);
            const read = url.searchParams.has("q");
            void route.fulfill(
              json({
                items: assets.slice(0, 12).map((a) => ({ ...a, score: null, rating: 0, archived: false, hidden: false, edited: false })),
                next_cursor: null,
                ranked: false,
                total: 12,
                parsed: read
                  ? {
                      text: "",
                      chips: [
                        { kind: "date", label: "2023", matched: "2023", from: "2023-01-01", to: "2023-12-31" },
                        { kind: "people", label: `${LONG} and Person B`, matched: `${LONG} and Person B`, people: [{ id: id(1), name: LONG }, { id: id(2), name: "Person B" }] },
                      ],
                    }
                  : null,
              }),
            );
            return true;
          }
          if (p === "/assets/search/facets") {
            void route.fulfill(json(FACETS));
            return true;
          }
          if (p === "/assets/albums" && method === "POST") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ id: "al1", title: "Both", description: null, photo_count: 0, cover: null, first_at: null, last_at: null, created_at: "", updated_at: "" }, 201));
            return true;
          }
          if (p === "/assets/albums/al1/assets" && method === "POST") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ count: 12 }));
            return true;
          }
          if (p === "/assets/albums/preview") {
            void route.fulfill(json({ count: 1234, capped: false, items: [], rules: RULES }));
            return true;
          }
          if (p === "/assets/tags" && method === "GET") {
            void route.fulfill(json({ items: [] }));
            return true;
          }
          return false;
        },
      ];
      assets = await setupApp(page, { handlers });

      // The people filter in the address: its chip names everyone, the results are the filter-only search.
      await page.goto(`/search?people=${id(1)},${id(2)}&from=2023-01-01&to=2023-12-31`);
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByRole("list", { name: "Active filters" }).getByText(`${LONG} and Person B`).waitFor();
      expect(searches.at(-1)).toContain(`people=${encodeURIComponent(`${id(1)},${id(2)}`)}`);
      await check("search-people-chip");

      // The filter sheet: the People group with a long name, the narrowing box, and the note.
      await page.getByRole("button", { name: /Filters/ }).click();
      const sheet = page.getByRole("dialog", { name: "Filters" });
      await sheet.getByRole("group", { name: "People" }).waitFor();
      await sheet.getByText("more: type a name to find them.").waitFor();
      await check("filters-people");
      await sheet.getByRole("searchbox", { name: "Find a person" }).fill("Person B");
      await sheet.getByRole("checkbox", { name: /^Person B/ }).waitFor();
      await check("filters-people-narrowed");
      await page.keyboard.press("Escape");

      // What was read out of a query: the people chip, long.
      await page.goto("/search?q=" + encodeURIComponent(`${LONG} and Person B 2023`));
      const understood = page.getByRole("list", { name: "Understood from your search" });
      await understood.getByText(`${LONG} and Person B`).waitFor();
      await check("search-parsed-people");

      // Save the results as an album, through the filters (no ids).
      await page.goto(`/search?people=${id(1)},${id(2)}`);
      await page.getByRole("button", { name: "Save as album" }).click();
      const save = page.getByRole("dialog", { name: "Save 12 photos as an album" });
      await save.waitFor();
      await check("search-save-album");
      await save.getByLabel("Album title").fill("Both");
      await save.getByRole("button", { name: "Create album" }).click();
      await page.getByText("Created Both with 12 photos").waitFor();
      expect(calls.at(-1)).toEqual({ method: "POST", path: "/assets/albums/al1/assets", body: { scope: { search: { people: `${id(1)},${id(2)}` } }, except: [] } });
      await check("search-saved-toast");

      // The People rule of a smart album.
      await page.goto("/albums");
      await page.getByRole("button", { name: "New smart album" }).click();
      const editor = page.getByRole("dialog", { name: "New smart album" });
      await editor.waitFor();
      await editor.getByLabel("Kind of rule to add").selectOption("people");
      await editor.getByRole("button", { name: "Add rule" }).click();
      const group = editor.getByRole("group", { name: /People together/ });
      await group.waitFor();
      await group.getByRole("checkbox", { name: /^Person B/ }).check();
      await group.getByRole("checkbox", { name: new RegExp(`^${LONG.slice(0, 20)}`) }).check();
      await editor.getByText(/1,234 photos/).waitFor();
      await check("smart-editor-people");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
