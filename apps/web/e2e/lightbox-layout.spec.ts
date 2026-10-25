// The photo viewer's bar at phone and desktop widths: Close, the counter and every icon button on one row, the actions
// (Download, Share, More) below with nothing clipped, and the More menu fully on screen. On desktop every action stays inline.
import { expect, test, type Locator, type Page } from "@playwright/test";
import { makeCheck, setupApp, shotPrefix, viewportOf } from "./kit";

/** Every visible button or link in `scope` whose text is wider than its box, and the ones that stick out of the viewport. */
async function clipped(scope: Locator) {
  return scope.evaluate((root) => {
    const bad: string[] = [];
    for (const el of Array.from(root.querySelectorAll<HTMLElement>("button, a.btn, .lightbox-count"))) {
      const b = el.getBoundingClientRect();
      if (b.width === 0 || b.height === 0) continue;
      const name = `${el.className.split(" ").slice(0, 2).join(".")} "${(el.textContent ?? "").trim().slice(0, 20)}"`;
      if (el.scrollWidth > el.clientWidth) bad.push(`${name} scrollWidth ${el.scrollWidth} > ${el.clientWidth}`);
      if (b.left < -0.5 || b.right > window.innerWidth + 0.5) bad.push(`${name} outside the viewport [${b.left}..${b.right}]`);
    }
    return bad;
  });
}

/** The distinct vertical centres of the bar's visible direct controls (Close, counter, icons) - one value means one row. */
const topRow = (bar: Locator) =>
  bar.evaluate((el) => {
    const rows = new Set<number>();
    for (const c of Array.from(el.children)) {
      if (c.classList.contains("lightbox-actions") || c.classList.contains("lightbox-right")) continue;
      const b = c.getBoundingClientRect();
      if (b.width) rows.add(Math.round(b.top + b.height / 2));
    }
    for (const c of Array.from(el.querySelector(".lightbox-right")?.children ?? [])) {
      if (c.classList.contains("lightbox-actions")) continue;
      const b = c.getBoundingClientRect();
      if (b.width) rows.add(Math.round(b.top + b.height / 2));
    }
    return Array.from(rows);
  });

async function openViewer(page: Page) {
  await page.goto("/library");
  await page.getByRole("button", { name: /Open IMG_00000/ }).first().click();
  const viewer = page.getByRole("dialog");
  await viewer.waitFor();
  return viewer;
}

for (const width of [320, 360, 390, 600]) {
  test.describe(`viewer bar ${width}px`, () => {
    test.use({ viewport: viewportOf(width), hasTouch: true });

    test("fits on one row, shows Download, Share and More, and the menu stays on screen", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("a6", width, "light"), problems);
      await setupApp(page);
      const viewer = await openViewer(page);
      const bar = viewer.locator(".lightbox-bar");
      const actions = viewer.locator(".lightbox-actions");
      await expect(actions.getByRole("button", { name: "More" })).toBeVisible();
      await check("bar");

      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      expect(await clipped(bar)).toEqual([]);
      expect(await topRow(bar), "Close, the counter and the icons share one line").toHaveLength(1);
      // Download, Share and More are what shows; the rest are in the menu.
      const visible = await actions.getByRole("button").or(actions.getByRole("link")).evaluateAll((els) =>
        els.filter((e) => e.getBoundingClientRect().width > 0).map((e) => (e.textContent ?? "").trim()),
      );
      expect(visible).toEqual(["Download", "Share", "More"]);

      const more = actions.getByRole("button", { name: "More" });
      await more.click();
      const menu = viewer.getByRole("menu");
      await expect(menu).toBeVisible();
      const items = menu.getByRole("menuitem");
      // Add to album, Play from here, Archive, Hide, Move to trash.
      await expect(items).toHaveCount(5);
      const vp = page.viewportSize()!;
      for (const it of await items.all()) {
        const b = (await it.boundingBox())!;
        expect(b.x).toBeGreaterThanOrEqual(0);
        expect(b.x + b.width).toBeLessThanOrEqual(vp.width);
        expect(b.y + b.height).toBeLessThanOrEqual(vp.height);
        expect(b.height).toBeGreaterThanOrEqual(43.5);
        expect(await it.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
      }
      await check("menu");
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);
      await expect(more).toBeFocused();
      await expect(viewer).toBeVisible();
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}

test.describe("viewer bar 1024px", () => {
  test.use({ viewport: { width: 1024, height: 740 } });

  test("keeps every action inline and hides More", async ({ page }) => {
    await setupApp(page);
    const viewer = await openViewer(page);
    const actions = viewer.locator(".lightbox-actions");
    await expect(actions.getByRole("button", { name: "More" })).toBeHidden();
    for (const name of ["Add to album", "Play from here", "Share", "Archive", "Hide", "Move to trash"]) {
      await expect(actions.getByRole("button", { name })).toBeVisible();
    }
    await expect(actions.getByText("Download", { exact: true })).toBeVisible();
    expect(await clipped(viewer.locator(".lightbox-bar"))).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
});
