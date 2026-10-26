// The photo viewer's chrome at phone and desktop widths: a top bar (back, when and where, the menu) and, on a phone, a bottom bar of
// icon buttons (on a desktop they sit top right); nothing clipped or off screen, the bars fade after a moment and come back on a tap
// (touch) or a mouse move, the menu stays on screen, and the info opens as a sheet (phone) or a side column that narrows the photo.
import { expect, test, type Locator, type Page } from "@playwright/test";
import { makeCheck, setupApp, shotPrefix, viewportOf } from "./kit";

const ACTIONS = ["Add to favorites", "Info", "Share", "Rotate and crop", "Move to trash"];

/** Every visible control in `scope` that sticks out of the viewport, is smaller than a finger, or has text wider than its box. */
async function problemsIn(scope: Locator, minSize = 38) {
  return scope.evaluate((root, min) => {
    const bad: string[] = [];
    for (const el of Array.from(root.querySelectorAll<HTMLElement>("button, a"))) {
      const b = el.getBoundingClientRect();
      if (b.width === 0 || b.height === 0) continue;
      const name = `"${(el.getAttribute("aria-label") ?? el.textContent ?? "").trim().slice(0, 24)}"`;
      if (el.scrollWidth > el.clientWidth + 1) bad.push(`${name} text wider than its box (${el.scrollWidth} > ${el.clientWidth})`);
      if (b.left < -0.5 || b.right > window.innerWidth + 0.5 || b.top < -0.5 || b.bottom > window.innerHeight + 0.5) bad.push(`${name} outside the viewport`);
      if (b.width < min || b.height < min) bad.push(`${name} is ${Math.round(b.width)}x${Math.round(b.height)}`);
    }
    return bad;
  }, minSize);
}

/** The distinct vertical centres of the buttons in `scope` (one value: one row). */
const rows = (scope: Locator) =>
  scope.evaluate((root) => {
    const ys = new Set<number>();
    for (const el of Array.from(root.querySelectorAll<HTMLElement>("button"))) {
      const b = el.getBoundingClientRect();
      if (b.width) ys.add(Math.round(b.top + b.height / 2));
    }
    return Array.from(ys);
  });

async function openViewer(page: Page) {
  await page.goto("/library");
  await page.getByRole("button", { name: /Open IMG_00000/ }).first().click();
  const viewer = page.getByRole("dialog", { name: "IMG_00000.jpg" });
  await viewer.waitFor();
  return viewer;
}

const opacityOf = (l: Locator) => l.evaluate((el) => Number(getComputedStyle(el).opacity));

for (const width of [320, 360, 390, 600]) {
  test.describe(`viewer chrome ${width}px`, () => {
    test.use({ viewport: viewportOf(width), hasTouch: true });

    test("top bar and bottom bar fit, the menu stays on screen, the bars fade and a tap brings them back", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("a6", width, "light"), problems);
      await setupApp(page);
      const viewer = await openViewer(page);
      const top = viewer.getByRole("button", { name: "Close" }).locator("xpath=..");
      await expect(viewer.getByRole("button", { name: "More" })).toBeVisible();
      await check("bars");

      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      expect(await problemsIn(top)).toEqual([]);
      expect(await rows(top), "Close and the menu share one line").toHaveLength(1);
      // The phone's bottom bar: every action as an icon, on one row.
      const bottom = viewer.getByRole("button", { name: "Info" }).locator("xpath=..");
      for (const name of ACTIONS) await expect(bottom.getByRole("button", { name })).toBeVisible();
      expect(await problemsIn(bottom)).toEqual([]);
      expect(await rows(bottom), "the actions share one line").toHaveLength(1);
      const [t, bb] = [await top.boundingBox(), await bottom.boundingBox()];
      expect(bb!.y, "the bottom bar sits below the top bar").toBeGreaterThan(t!.y + t!.height);
      // The title says the day, and the position of the photo among the library.
      await expect(top.getByText(/^\d+ \/ \d+$/)).toBeVisible();

      const more = viewer.getByRole("button", { name: "More" });
      await more.click();
      const menu = page.getByRole("menu");
      await expect(menu).toBeVisible();
      const items = menu.getByRole("menuitem");
      const vp = page.viewportSize()!;
      for (const it of await items.all()) {
        const b = (await it.boundingBox())!;
        expect(b.x).toBeGreaterThanOrEqual(0);
        expect(b.x + b.width).toBeLessThanOrEqual(vp.width);
        expect(b.y + b.height).toBeLessThanOrEqual(vp.height);
        expect(b.height).toBeGreaterThanOrEqual(38);
        expect(await it.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
      }
      await expect(menu.getByRole("menuitem", { name: "Add to album" })).toBeVisible();
      await expect(menu.getByRole("menuitem", { name: "Play from here" })).toBeVisible();
      await expect(menu.getByRole("menuitem", { name: "Archive" })).toBeVisible();
      await check("menu");
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);
      await expect(more).toBeFocused();
      await expect(viewer).toBeVisible();

      // The bars go away after a quiet spell (never while the keyboard is on a button) and a tap on the photo brings them back.
      await more.blur();
      await expect.poll(() => opacityOf(top), { timeout: 8000 }).toBe(0);
      await check("bars-hidden");
      await page.touchscreen.tap(Math.round(vp.width / 2), Math.round(vp.height / 2));
      await expect.poll(() => opacityOf(top), { timeout: 3000 }).toBe(1);
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}

test.describe("viewer chrome 1024px", () => {
  test.use({ viewport: { width: 1024, height: 740 } });

  test("the actions sit top right, the arrows are at the sides, and the info is a column that narrows the photo", async ({ page }) => {
    await setupApp(page);
    const viewer = await openViewer(page);
    const top = viewer.getByRole("button", { name: "Close" }).locator("xpath=..");
    for (const name of [...ACTIONS, "More"]) await expect(top.getByRole("button", { name })).toBeVisible();
    expect(await problemsIn(top)).toEqual([]);
    expect(await rows(top)).toHaveLength(1);
    await expect(viewer.getByRole("button", { name: "Next photo" })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

    const photo = viewer.locator(".viewer-stage");
    const before = (await photo.boundingBox())!;
    await viewer.getByRole("button", { name: "Info" }).click();
    const panel = page.locator("#details-panel");
    await expect(panel).toBeVisible();
    await expect.poll(async () => (await photo.boundingBox())!.width).toBeLessThan(before.width - 300);
    await expect.poll(async () => (await panel.boundingBox())!.width).toBe(384);
    const after = (await photo.boundingBox())!;
    const side = (await panel.boundingBox())!;
    expect(after.x + after.width, "the photo ends where the panel begins").toBeLessThanOrEqual(side.x + 1);
    await viewer.getByRole("button", { name: "Close info" }).click();
    await expect(panel).toHaveCount(0);
    await expect.poll(async () => (await photo.boundingBox())!.width).toBeGreaterThan(before.width - 2);
  });

  test("the bars fade when the mouse rests, and come back when it moves", async ({ page }) => {
    await setupApp(page);
    const viewer = await openViewer(page);
    const top = viewer.getByRole("button", { name: "Close" }).locator("xpath=..");
    await page.mouse.move(500, 400);
    await viewer.getByRole("button", { name: "Close" }).blur();
    await expect.poll(() => opacityOf(top), { timeout: 8000 }).toBe(0);
    await page.mouse.move(520, 420);
    await expect.poll(() => opacityOf(top), { timeout: 3000 }).toBe(1);
  });
});
