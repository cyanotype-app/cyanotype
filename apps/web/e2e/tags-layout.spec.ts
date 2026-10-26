// Tags at phone and desktop widths: the Tag dialog with its suggestions, the info panel's chips, the Tags view, a tag page with its
// dialogs and the filter sheet, all with very long names. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG = "A really quite long tag name with_an_unbroken_run_of_characters_that_could_overflow_" + "x".repeat(10);
const TAGS = [
  { id: "t1", name: LONG, count: 12345 },
  { id: "t2", name: "Beach", count: 40 },
  { id: "t3", name: "trip/2024/Lisbon", count: 7 },
  { id: "t4", name: "Sunset", count: 2 },
];

for (const { width, scheme } of RUNS) {
  test.describe(`tags ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("tag from the selection bar, the viewer, the Tags view and the filter without overflow", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("a2", width, scheme), problems);
      const applied: unknown[] = [];
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          if (p === "/assets/tags" && method === "GET") {
            const q = (url.searchParams.get("q") ?? "").toLowerCase();
            void route.fulfill(json({ items: TAGS.filter((t) => t.name.toLowerCase().includes(q)) }));
            return true;
          }
          if (p === "/assets/tags/apply") {
            applied.push(route.request().postDataJSON());
            void route.fulfill(json({ added: 2, removed: 0 }));
            return true;
          }
          if (p === "/assets/search/facets") {
            void route.fulfill(json({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [] }));
            return true;
          }
          if (p === "/assets/search" && method === "GET") {
            void route.fulfill(json({ items: [], next_cursor: null, ranked: false, total: 0 }));
            return true;
          }
          return false;
        },
      ];
      const assets = await setupApp(page, { handlers });
      // The open photo carries tags, one imported and one with a very long name.
      await page.route(/\/assets\/a2$/, (route) =>
        route.fulfill(
          json({
            ...assets[2],
            albums: [],
            original_url: assets[2]!.thumb_url,
            preview_url: assets[2]!.thumb_url,
            tags: [
              { id: "t1", name: LONG, imported: false },
              { id: "t2", name: "Beach", imported: true },
              { id: "t3", name: "trip/2024/Lisbon", imported: false },
            ],
          }),
        ),
      );

      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("library-select-tag");
      await page.getByRole("button", { name: "Tag", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: /Tag 2 photos/ });
      await dialog.waitFor();
      await dialog.getByRole("combobox", { name: "Add tags" }).fill("be");
      await page.getByRole("option", { name: /Beach/ }).waitFor();
      await check("tag-dialog-suggestions");
      await page.getByRole("option", { name: /Beach/ }).click();
      await dialog.getByRole("combobox", { name: "Add tags" }).fill(LONG.slice(0, 40));
      await page.getByRole("option").first().click();
      await check("tag-dialog-chips");
      await dialog.getByRole("button", { name: "Apply" }).click();
      await page.getByText(/^Added /).waitFor();
      expect((applied.at(-1) as { add: string[] }).add).toEqual(["Beach", LONG]);
      await check("tagged-toast");

      await page.getByRole("button", { name: /Open IMG_00002/ }).first().click();
      await page.getByRole("dialog", { name: "IMG_00002.jpg" }).waitFor();
      await page.keyboard.press("i");
      const panel = page.locator("#details-panel");
      await panel.getByRole("button", { name: "Add tag" }).waitFor();
      await check("lightbox-info-tags");
      await panel.getByRole("button", { name: "Add tag" }).click();
      await page.getByRole("dialog", { name: /Tag 1 photo/ }).waitFor();
      await check("lightbox-add-tag-dialog");
      await page.keyboard.press("Escape");

      await page.goto("/tags");
      await page.getByRole("link", { name: /Beach/ }).waitFor();
      await check("tags-view");
      await page.goto("/tags/t1");
      await page.getByRole("heading", { name: LONG }).waitFor();
      await check("tag-page");
      await page.getByRole("button", { name: "Merge into…" }).click();
      await page.getByRole("dialog", { name: /Merge/ }).waitFor();
      await check("tag-merge-dialog");
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Delete tag" }).click();
      await page.getByRole("alertdialog").waitFor();
      await check("tag-delete-dialog");
      await page.keyboard.press("Escape");

      await page.goto(`/search?${new URLSearchParams({ tags: `${LONG},Beach`, tag_mode: "any" })}`);
      await page.getByRole("button", { name: /^Filters/ }).click();
      const sheet = page.getByRole("dialog", { name: "Filters" });
      await sheet.getByRole("group", { name: "Match" }).waitFor();
      await check("filter-sheet-tags");
      await sheet.getByRole("button", { name: "Show photos" }).click();
      await page.getByRole("list", { name: "Active filters" }).getByText(/^Tags: /).waitFor();
      await check("search-tags-chip");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
