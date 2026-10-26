// The Photos page at phone and desktop widths: square crops on a phone, justified rows on a desktop, filling the full width
// (no gap at the right edge), the
// header's Select button and view menu, the selection bar, and the empty states. Nothing may stick out sideways. Set SHOT_DIR
// to save the screenshots.
import { expect, test, type Page } from "@playwright/test";
import { makeCheck, RUNS, setupApp, shotPrefix, viewportOf } from "./kit";

/** Every full row of tiles starts at the left edge of the page and ends at its right edge. */
async function expectFilled(page: Page, label: string) {
  const r = await page.evaluate(() => {
    // The grid spans its column: the whole page on a phone, everything right of the sidebar on a desktop.
    const box = document.querySelector("[data-timeline] [role=list]")!.getBoundingClientRect();
    const w = box.right;
    if (Math.abs(box.right - document.documentElement.clientWidth) > 0.5) return { w: -1, x0: 0, out: [] }; // not flush with the page's right edge
    const x0 = box.left;
    const rows = Array.from(document.querySelectorAll<HTMLElement>("[data-row=tiles]"));
    const out: { left: number; right: number; n: number; fill: boolean }[] = [];
    for (const row of rows) {
      const cells = Array.from(row.children).map((c) => c.getBoundingClientRect());
      if (cells.length === 0) continue;
      out.push({ left: Math.min(...cells.map((c) => c.left)), right: Math.max(...cells.map((c) => c.right)), n: cells.length, fill: row.hasAttribute("data-fill") });
    }
    return { w, x0, out };
  });
  expect(r.w, `${label}: the grid reaches the right edge of the page`).toBeGreaterThan(0);
  expect(r.out.length, `${label}: rows`).toBeGreaterThan(1);
  const full = r.out.filter((x) => x.fill);
  expect(full.length, `${label}: full rows`).toBeGreaterThan(0);
  for (const x of full) {
    expect(Math.abs(x.left - r.x0), `${label}: left edge`).toBeLessThan(0.6);
    expect(Math.abs(x.right - r.w), `${label}: right edge (${x.right} of ${r.w})`).toBeLessThan(0.6);
  }
}

for (const { width, scheme } of RUNS) {
  test.describe(`photos ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });
    const desktop = width >= 768;

    test("the grid fills the width: squares on a phone, justified rows on a desktop", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("photos", width, scheme), problems);
      const assets = await setupApp(page, { count: 90 });
      // Portraits, squares and wide ones, so the justified rows have something to fit.
      assets.forEach((a, i) => {
        const shapes = [[4000, 3000], [3000, 4000], [4000, 2250], [3000, 3000], [4000, 3000], [2000, 3000]] as const;
        [a.width, a.height] = shapes[i % shapes.length]!;
      });

      await page.goto("/photos");
      await expect(page.getByRole("heading", { level: 1, name: "Photos" })).toBeVisible();
      await expect(page.getByRole("button", { name: /Open IMG_00000/ })).toBeVisible();
      await expect(page.locator("[data-tile] img").first()).toBeVisible();
      // Old rows of buttons are gone: no pills, no big upload buttons on the page.
      await expect(page.getByRole("link", { name: "All photos" })).toHaveCount(0);
      await expect(page.getByRole("button", { name: "Upload folder" })).toHaveCount(0);
      const style = desktop ? "justified" : "square";
      // Square rows are equal columns; justified ones are as wide as each photo's shape asks.
      const equal = await page.evaluate(() => {
        const cols = getComputedStyle(document.querySelector("[data-row=tiles]")!).gridTemplateColumns.split(" ").map(parseFloat);
        return { n: cols.length, same: Math.max(...cols) - Math.min(...cols) < 0.5 };
      });
      expect(equal.same, `${style}: equal columns`).toBe(!desktop);
      if (width === 390) expect(equal.n).toBe(3);
      await expectFilled(page, style);
      await check(style);

      // The view menu: size, sort, stacks, slideshow stay reachable; the screen picks the layout.
      await page.getByRole("button", { name: "View options" }).click();
      const menu = page.getByRole("menu");
      await expect(menu.getByRole("menuitemradio", { name: "Justified" })).toHaveCount(0);
      await expect(menu.getByRole("menuitem", { name: "Larger photos" })).toBeVisible();
      await expect(menu.getByRole("menuitemradio", { name: "Date taken" })).toBeVisible();
      await expect(menu.getByRole("menuitemradio", { name: "Stacked" })).toBeVisible();
      await check("view-menu");
      await page.keyboard.press("Escape");

      // Scrolled: the month header stays pinned under the page's header.
      await page.evaluate(() => window.scrollTo(0, 700));
      await page.waitForTimeout(250);
      const pinned = page.locator("[data-row=month][data-pinned]");
      await expect(pinned).toHaveCount(1);
      const hdr = await page.evaluate(() => document.querySelector("header.sticky")!.getBoundingClientRect().height);
      expect(Math.abs((await pinned.boundingBox())!.y - hdr)).toBeLessThan(1.5);
      await check(`${style}-scrolled`);
      await page.evaluate(() => window.scrollTo(0, 0));

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("selecting: the bar replaces the tab bar on a phone and floats on a desktop", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("photos-sel", width, scheme), problems);
      await setupApp(page, { count: 60 });
      await page.goto("/photos");
      await expect(page.getByRole("button", { name: /Open IMG_00000/ })).toBeVisible();

      await page.getByRole("button", { name: "Select", exact: true }).click();
      const bar = page.getByRole("toolbar", { name: "Selection" });
      await expect(bar).toBeVisible();
      await expect(bar.getByText("Select photos")).toBeVisible();
      await page.getByRole("button", { name: "Select IMG_00001.jpg" }).click();
      await page.getByRole("button", { name: "Select IMG_00002.jpg" }).click();
      await expect(bar.getByText("2 selected")).toBeVisible();
      await page.waitForTimeout(400);
      await check("selected");
      const box = (await bar.boundingBox())!;
      const vp = page.viewportSize()!;
      if (desktop) {
        expect(box.y + box.height).toBeLessThan(vp.height - 8); // floating, not docked
        expect(box.width).toBeLessThan(vp.width - 40);
      } else {
        expect(Math.abs(box.y + box.height - vp.height)).toBeLessThan(1.5); // docked over the tab bar
        expect(box.width).toBeGreaterThan(vp.width - 1);
      }
      // Every action is reachable: on the bar, or behind its "More" menu.
      await expect(bar.getByRole("button", { name: "Favorite" })).toBeVisible();
      await expect(bar.getByRole("button", { name: "Move to trash" })).toBeVisible();
      await bar.getByRole("button", { name: "More actions" }).click();
      const more = page.getByRole("menu", { name: "More actions" });
      await expect(more.getByRole("menuitem").first()).toBeVisible();
      await check("selection-more");
      const names = await more.getByRole("menuitem").allTextContents();
      const inline = await bar.getByRole("button").allTextContents();
      const all = [...names, ...inline].join(" ");
      for (const label of ["Share", "Download", "Tag", "Rate", "Stack", "Archive", "Hide", "Edit"]) expect(all, label).toContain(label);
      await page.keyboard.press("Escape");

      // A toast sits above the bar.
      await bar.getByRole("button", { name: "Favorite" }).click();
      const toast = page.getByText("2 added to favorites");
      await expect(toast).toBeVisible();
      await page.waitForTimeout(300);
      await check("toast");
      await expect(bar).toBeHidden();

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("empty states: an icon, a line, a hint and the way forward", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("photos-empty", width, scheme), problems);
      await setupApp(page, { count: 0 });
      await page.goto("/photos");
      await expect(page.getByRole("heading", { name: "Your library is empty" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Upload photos" })).toBeVisible();
      await check("library");
      for (const [path, title] of [
        ["/favorites", "No favorites yet"],
        ["/archive", "The archive is empty"],
        ["/recent", "Nothing added yet"],
      ] as const) {
        await page.goto(path);
        await expect(page.getByRole("heading", { name: title })).toBeVisible();
        await check(path.slice(1));
      }
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
