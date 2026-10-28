// The one "Show faces and pets on the photo" switch in the viewer's Info panel, with Pets on: it outlines every animal with a label (the pet's
// name, else Dog or Cat) together with the faces, at fit and zoomed; a tap on a named pet's box opens its page and on any other the
// "Add to pet" sheet. At phone and desktop widths, light and dark. Set SHOT_DIR to save the screenshots.
import { expect, test, type Locator, type Page } from "@playwright/test";
import { json, makeCheck, RUNS, shotPrefix, viewportOf } from "./kit";
import { openPhoto, setupViewer } from "./viewerKit";

type Box = [number, number, number, number];
const PET: { id: string; box: Box; pet: { id: string; name: string } | null; kind: "dog" | "cat" }[] = [
  { id: "x1", kind: "dog", box: [0.08, 0.1, 0.34, 0.4], pet: { id: "p1", name: "Biscuit" } },
  { id: "x2", kind: "cat", box: [0.55, 0.45, 0.3, 0.35], pet: null },
];
const FACE = { id: "fn", box: [0.35, 0.55, 0.12, 0.18], person: null };

async function setup(page: Page, withFaces: boolean) {
  await setupViewer(page);
  await page.route("**/assets/pets/status", (r) => r.fulfill(json({ enabled: true })));
  await page.route(/\/assets\/pets$/, (r) => r.fulfill(json({ any: 2, kinds: [], scanned: 2, total: 2 })));
  await page.route(/\/assets\/pets\/named$/, (r) => r.fulfill(json({ items: [{ id: "p1", name: "Biscuit", kind: "dog", hidden: false, photos: 3, detections: 3, pending: 0, cover: null }] })));
  await page.route(/\/assets\/a\d+\/pets$/, (r) => r.fulfill(json({ items: r.request().url().endsWith("/assets/a0/pets") ? PET : [], scanned: true })));
  await page.route("**/assets/people/status", (r) => r.fulfill(json({ enabled: withFaces })));
  await page.route(/\/assets\/people$/, (r) => r.fulfill(json({ items: [] })));
  await page.route(/\/assets\/a\d+\/faces$/, (r) => r.fulfill(json({ items: withFaces && r.request().url().endsWith("/assets/a0/faces") ? [FACE] : [], dismissed: [] })));
}

/** A click with the mouse, or a tap with one finger (the viewer hears both itself; a plain touch tap is not what a finger sends). */
async function press(page: Page, target: Locator, width: number) {
  const b = (await target.boundingBox())!;
  const p = { x: b.x + b.width / 2, y: b.y + b.height / 2 };
  if (width >= 600) return page.mouse.click(p.x, p.y);
  const cdp = await page.context().newCDPSession(page);
  await Promise.all([cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: p.x, y: p.y, id: 0 }] }), cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] })]);
}

const pets = (page: Page) => page.locator(".pet-regions .face-region");

for (const { width, scheme } of RUNS) {
  test.describe(`pet boxes ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600, isMobile: width < 600 });

    test("the switch outlines the faces and the pets together, with labels, and follows the zoom", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("petboxes", width, scheme), problems);
      await setup(page, true);
      await openPhoto(page, 0);
      await expect(pets(page)).toHaveCount(2);
      await page.mouse.move(2, 2);
      await expect(page.getByRole("link", { name: "Biscuit: open their page" })).toHaveCount(1);
      await expect(page.getByRole("button", { name: "Cat: add to a pet" })).toHaveCount(1);
      await expect(page.locator(".pet-regions .region-label").first()).toHaveCSS("opacity", "0");

      await page.keyboard.press("i");
      const panel = page.locator("#details-panel");
      const sw = panel.getByRole("switch", { name: "Show faces and pets on the photo" });
      await sw.waitFor();
      await expect(panel.getByRole("switch")).toHaveCount(1);
      await check("info");
      await sw.check();
      await expect(page.locator(".pet-regions .region-label").first()).toHaveCSS("opacity", "1");
      expect(await page.locator(".pet-regions .region-label").allTextContents()).toEqual(["Biscuit", "Cat"]);
      expect(await page.locator(".face-regions.all").count(), "faces and pets both outlined").toBe(2);
      if (width >= 600) await check("on");
      else {
        await page.keyboard.press("Escape");
        await check("on");
      }

      if (width >= 600) {
        for (let i = 0; i < 2; i++) await page.keyboard.press("+");
        await page.waitForTimeout(500);
        await check("on-zoomed");
        await page.keyboard.press("z");
      }
      if (width >= 600) {
        await sw.uncheck();
        await expect(page.locator(".pet-regions .region-label").first()).toHaveCSS("opacity", "0");
        expect(await page.locator(".face-regions.all").count()).toBe(0);
      }
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("a tap on a named pet's box opens its page; on any other, the Add to pet sheet", async ({ page }) => {
      await setup(page, false);
      await openPhoto(page, 0);
      if (width >= 600) {
        await page.keyboard.press("i");
        const panel = page.locator("#details-panel");
        await expect(panel.getByRole("switch", { name: "Show pets on the photo" })).toHaveCount(1);
        await expect(panel.getByRole("switch")).toHaveCount(1);
        await page.keyboard.press("i");
      }
      await press(page, page.getByRole("button", { name: "Cat: add to a pet" }), width);
      await page.getByRole("dialog", { name: "Add to pet" }).waitFor();
      await page.keyboard.press("Escape");
      await press(page, page.getByRole("link", { name: "Biscuit: open their page" }), width);
      await expect(page).toHaveURL(/\/pets\/p1$/);
    });
  });
}
