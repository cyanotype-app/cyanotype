// Smart albums at phone and desktop widths: the Albums list with smart cards, the rule editor with every kind of rule and long values,
// a smart album's page and its selection bar. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG = "Summer road trip along the entire Pacific coast highway with the whole extended family 2019";
const RULES = {
  match: "any",
  archived: "exclude",
  rules: [
    { field: "date", from: "2019-01-01", to: "2024-12-31" },
    { field: "place", value: "Llanfairpwllgwyngyllgogerychwyrndrobwllllantysiliogogogoch" },
    { field: "camera", value: "Hasselblad X2D 100C with the XCD 4/45P lens and the optional extension tube set" },
    { field: "type", value: "raw" },
    { field: "favorite", value: true },
    { field: "rating", min: 4 },
    { field: "tags", names: ["trip/2024/Lisbon", "A really quite long tag name with_an_unbroken_run_of_characters_xxxxxxxxxxxxxxxxxxxxxx"], mode: "any" },
    { field: "has_location", value: false },
    { field: "has_text", value: true },
    { field: "caption", text: "birthday cake with_an_unbroken_run_of_characters_that_could_overflow_xxxxxxxxxxx" },
  ],
};
const smartAlbum = (id: string, title: string, count: number, capped = false) => ({
  id,
  title,
  description: null,
  smart: true,
  rules: RULES,
  photo_count: count,
  photo_count_capped: capped,
  hidden_count: 0,
  cover: { id: "a1", thumbhash: null, thumb_url: "/fixture-thumbs/1.svg" },
  first_at: "2019-01-01T00:00:00Z",
  last_at: "2024-06-30T00:00:00Z",
  created_at: "2024-01-01T00:00:00Z",
  updated_at: "2024-01-01T00:00:00Z",
});
const plainAlbum = { ...smartAlbum("p1", "Pets", 12), smart: false, rules: null };

for (const { width, scheme } of RUNS) {
  test.describe(`smart albums ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("list, editor and album page without overflow", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("a3", width, scheme), problems);
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          if (p === "/assets/albums" && method === "GET") {
            void route.fulfill(json({ items: [smartAlbum("s1", LONG, 10000, true), smartAlbum("s2", "Faves", 12), plainAlbum] }));
            return true;
          }
          if (p === "/assets/albums/preview") {
            void route.fulfill(json({ count: 1234, capped: false, items: [], rules: RULES }));
            return true;
          }
          if (/^\/assets\/albums\/s\d$/.test(p) && method === "GET") {
            void route.fulfill(json(smartAlbum("s1", LONG, 10000, true)));
            return true;
          }
          if (/^\/assets\/albums\/s\d\/assets$/.test(p)) {
            void route.fulfill(json({ items: [], next_cursor: null, next_before: null }));
            return true;
          }
          if (p === "/assets/search/facets") {
            void route.fulfill(json({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [] }));
            return true;
          }
          if (p === "/assets/tags" && method === "GET") {
            void route.fulfill(json({ items: [] }));
            return true;
          }
          return false;
        },
      ];
      const assets = await setupApp(page, { handlers });
      // The album page lists photos: the library's own fake photos.
      await page.route(/\/assets\/albums\/s1\/assets/, (route) =>
        route.fulfill(json({ items: assets.slice(0, 12), next_cursor: null, next_before: null })),
      );

      await page.goto("/albums");
      await page.getByRole("link", { name: /Faves/ }).first().waitFor();
      await check("albums-with-smart");
      await page.getByRole("button", { name: "New", exact: true }).click();
      await check("albums-new-menu");
      await page.getByRole("menuitem", { name: "New smart album" }).click();
      const editor = page.getByRole("dialog", { name: "New smart album" });
      await editor.waitFor();
      await check("smart-editor-new");
      await page.keyboard.press("Escape");

      await page.goto("/albums/s1");
      await page.getByRole("heading", { name: /Summer road trip/ }).waitFor();
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await check("smart-album-page");
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await check("smart-album-select");
      await page.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Edit rules" }).click();
      const dialog = page.getByRole("dialog", { name: "Edit rules" });
      await dialog.waitFor();
      await dialog.getByText(/1,234 photos/).waitFor();
      await check("smart-editor-rules");
      await dialog.getByRole("button", { name: "Save" }).scrollIntoViewIfNeeded();
      await check("smart-editor-bottom");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
