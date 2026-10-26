// Star ratings at phone and desktop widths: nothing sticks out sideways in the library's Rate dialog, the viewer's stars
// and info panel, or the search filter with its chip. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

for (const { width, scheme } of RUNS) {
  test.describe(`ratings ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("rate from the selection bar, the viewer and the filter sheet without overflow", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("a1", width, scheme), problems);
      const rated: { ids?: string[]; rating: number }[] = [];
      let assets: Record<string, unknown>[] = [];

      const handlers: Handler[] = [
        (route, url, method) => {
          if (url.pathname === "/assets/rating" && method === "POST") {
            const body = route.request().postDataJSON() as { ids?: string[]; rating: number };
            rated.push(body);
            for (const a of assets) if (body.ids?.includes(a.id as string)) a.rating = body.rating;
            void route.fulfill(json({ count: body.ids?.length ?? 0 }));
            return true;
          }
          if (url.pathname === "/assets/search/facets") {
            void route.fulfill(
              json({
                total: 30,
                favorites: 3,
                with_location: 10,
                ratings: [
                  { value: "5", count: 4 },
                  { value: "4", count: 9 },
                  { value: "3", count: 21 },
                  { value: "2", count: 26 },
                  { value: "1", count: 30 },
                ],
                types: [{ value: "jpeg", count: 30 }],
                cameras: [],
                lenses: [],
                places: [],
                years: [{ value: "2026", count: 30 }],
              }),
            );
            return true;
          }
          if (url.pathname === "/assets/search" && method === "GET") {
            const min = Number(url.searchParams.get("rating") ?? 0);
            const items = assets.filter((a) => ((a.rating as number) ?? 0) >= min).slice(0, 12);
            void route.fulfill(json({ items, next_cursor: null, ranked: false, total: items.length }));
            return true;
          }
          return false;
        },
      ];
      assets = (await setupApp(page, { handlers })) as unknown as Record<string, unknown>[];
      assets.forEach((a, i) => (a.rating = i % 6));

      // The library: select two photos and rate them.
      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await check("library");
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("library-select-rate");
      await page.getByRole("button", { name: "Rate", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Rate photos" });
      await dialog.waitFor();
      await check("rate-dialog");
      await dialog.getByRole("button", { name: /4 stars/ }).click();
      await page.getByText("2 photos rated 4 stars").waitFor();
      expect(rated.at(-1)?.rating).toBe(4);
      expect(rated.at(-1)?.ids).toHaveLength(2);
      await check("rated-toast");

      // The viewer: the stars under the photo, a digit key, the row in the info panel.
      await page.getByRole("button", { name: /Open IMG_00002/ }).first().click();
      await page.getByRole("dialog", { name: "IMG_00002.jpg" }).waitFor();
      await expect(page.getByRole("group", { name: /Rating of IMG_00002.jpg: 2 stars/ })).toBeVisible();
      await page.keyboard.press("3");
      await expect(page.getByRole("group", { name: /Rating of IMG_00002.jpg: 3 stars/ })).toBeVisible();
      expect(rated.at(-1)).toEqual({ ids: ["a2"], rating: 3 });
      await check("lightbox-stars");
      await page.keyboard.press("i");
      await page.locator("#details-panel").waitFor();
      await expect(page.locator("#details-panel").getByText("Rating", { exact: true })).toBeVisible();
      await check("lightbox-info-rating");
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await expect(page).toHaveURL(/\/library/);

      // The filter: the sheet with its counts and the chip it leaves behind.
      await page.goto("/search");
      await page.getByRole("button", { name: /^Filters/ }).click();
      const sheet = page.getByRole("dialog", { name: "Filters" });
      await sheet.getByLabel("Rating").waitFor();
      await sheet.getByLabel("Rating").selectOption("4");
      await check("filter-sheet-rating");
      await sheet.getByRole("button", { name: "Show photos" }).click();
      await page.getByRole("list", { name: "Active filters" }).getByText("4 stars & up").waitFor();
      await expect(page).toHaveURL(/rating=4/);
      await check("search-rating-chip");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
