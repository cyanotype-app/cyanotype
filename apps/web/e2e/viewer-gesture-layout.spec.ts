// The viewer's touch gestures and its open and close motion in a real browser: drag sideways and the neighbour follows the finger (and
// settles on release, or springs back from a short drag), drag down and the photo shrinks while the backdrop fades (then closes, or
// springs back), the photo grows out of its tile in the grid and shrinks back into it, and with less motion wanted it only fades.
import { expect, test, type CDPSession, type Page } from "@playwright/test";
import { openPhoto, setupViewer } from "./viewerKit";

type Pt = { x: number; y: number };

async function fingers(cdp: CDPSession, type: "touchStart" | "touchMove" | "touchEnd", pts: Pt[]) {
  await cdp.send("Input.dispatchTouchEvent", { type, touchPoints: pts.map((p, id) => ({ x: p.x, y: p.y, id })) });
}

const pause = (page: Page, ms: number) => page.waitForTimeout(ms);

/** The photo's current slot and its neighbours (the paging strip), as boxes on screen. */
const slots = (page: Page) =>
  page.evaluate(() =>
    Array.from(document.querySelectorAll<HTMLElement>(".viewer-slot")).map((s) => {
      const img = s.querySelector("img");
      const b = img?.getBoundingClientRect();
      return { current: !!s.querySelector(".viewer-photo.current"), src: img?.getAttribute("src") ?? "", left: b?.left ?? 0, right: b?.right ?? 0, width: b?.width ?? 0 };
    }),
  );

/** The lift of the photo box: its scale, and the backdrop's opacity. */
const lift = (page: Page) =>
  page.evaluate(() => {
    const photo = document.querySelector<HTMLElement>(".viewer-stage")!.parentElement!;
    const m = new DOMMatrix(getComputedStyle(photo).transform);
    const backdrop = document.querySelector<HTMLElement>('[role="dialog"][aria-modal="true"] > div[aria-hidden="true"]')!;
    return { scale: m.a, y: m.f, backdrop: Number(getComputedStyle(backdrop).opacity) };
  });

test.describe("gestures on a phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

  test("a drag sideways pages: the neighbour follows the finger, a long drag settles on it, a short one springs back", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const cdp = await page.context().newCDPSession(page);
    const y = 400;
    // Half way across: the next photo is on screen next to this one, already loaded.
    await fingers(cdp, "touchStart", [{ x: 320, y }]);
    for (let i = 1; i <= 6; i++) await fingers(cdp, "touchMove", [{ x: 320 - i * 30, y }]);
    const mid = await slots(page);
    const cur = mid.find((s) => s.current)!;
    const next = mid.find((s) => !s.current && s.left > cur.left)!;
    expect(cur.left, "this photo moved with the finger").toBeLessThan(0);
    expect(next.left, "the next photo is coming in").toBeLessThan(390);
    expect(next.src).toContain("portrait");
    await fingers(cdp, "touchEnd", []);
    await expect(page.locator(".viewer-photo.current img")).toHaveAttribute("src", /portrait\.svg/);
    await expect(page.getByRole("dialog", { name: "IMG_00001.jpg" })).toBeVisible();
    // After settling the strip is at rest: the photo fills its place again.
    await expect.poll(async () => Math.round((await slots(page)).find((s) => s.current)!.left)).toBeGreaterThanOrEqual(0);

    // A short, slow drag goes back to the same photo.
    await fingers(cdp, "touchStart", [{ x: 200, y }]);
    for (let i = 1; i <= 4; i++) {
      await fingers(cdp, "touchMove", [{ x: 200 + i * 8, y }]);
      await pause(page, 60);
    }
    await pause(page, 300);
    await fingers(cdp, "touchEnd", []);
    await pause(page, 600);
    await expect(page.getByRole("dialog", { name: "IMG_00001.jpg" })).toBeVisible();

    // Back with a drag the other way.
    await fingers(cdp, "touchStart", [{ x: 60, y }]);
    for (let i = 1; i <= 6; i++) await fingers(cdp, "touchMove", [{ x: 60 + i * 45, y }]);
    await fingers(cdp, "touchEnd", []);
    await expect(page.getByRole("dialog", { name: "IMG_00000.jpg" })).toBeVisible();
  });

  test("the first photo gives way a little past its edge and springs back", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const cdp = await page.context().newCDPSession(page);
    await fingers(cdp, "touchStart", [{ x: 80, y: 400 }]);
    for (let i = 1; i <= 6; i++) await fingers(cdp, "touchMove", [{ x: 80 + i * 50, y: 400 }]);
    const cur = (await slots(page)).find((s) => s.current)!;
    expect(cur.left, "pulled, but by a third").toBeGreaterThan(0);
    expect(cur.left).toBeLessThan(150);
    await fingers(cdp, "touchEnd", []);
    await expect(page.getByRole("dialog", { name: "IMG_00000.jpg" })).toBeVisible();
    await expect.poll(async () => Math.round((await slots(page)).find((s) => s.current)!.left)).toBeLessThan(1);
  });

  test("a drag down shrinks the photo and fades the backdrop, then closes; a short one springs back", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const cdp = await page.context().newCDPSession(page);
    await expect.poll(async () => (await lift(page)).backdrop).toBeGreaterThan(0.99); // the opening fade is done
    // Short and slow: back to rest.
    await fingers(cdp, "touchStart", [{ x: 195, y: 300 }]);
    for (let i = 1; i <= 4; i++) {
      await fingers(cdp, "touchMove", [{ x: 195, y: 300 + i * 10 }]);
      await pause(page, 80);
    }
    await pause(page, 300);
    await fingers(cdp, "touchEnd", []);
    await expect.poll(async () => (await lift(page)).scale).toBeGreaterThan(0.999);
    await expect.poll(async () => (await lift(page)).backdrop).toBeGreaterThan(0.99);

    // Long: it follows the finger, smaller and fainter, and lets go of the viewer.
    await fingers(cdp, "touchStart", [{ x: 195, y: 300 }]);
    for (let i = 1; i <= 5; i++) await fingers(cdp, "touchMove", [{ x: 195, y: 300 + i * 30 }]);
    const held = await lift(page);
    expect(held.y, "follows the finger").toBeGreaterThan(100);
    expect(held.scale, "smaller").toBeLessThan(1);
    expect(held.backdrop, "the backdrop fades with it").toBeLessThan(0.75);
    await fingers(cdp, "touchEnd", []);
    await expect(page.getByRole("dialog", { name: "IMG_00000.jpg" })).toHaveCount(0);
    await expect(page).toHaveURL(/\/(library|photos)/);
  });

  test("the photo grows out of its tile and shrinks back into it", async ({ page }) => {
    await setupViewer(page);
    await page.goto("/library");
    const tile = page.getByRole("button", { name: /Open IMG_00000/ }).first();
    await tile.waitFor();
    // The grid marks its tiles with data-asset-id; this build's tile is stood in for by a marked box where the real tile is.
    await page.evaluate(() => {
      const t = Array.from(document.querySelectorAll<HTMLElement>("button")).find((b) => /Open IMG_00000/.test(b.getAttribute("aria-label") ?? ""))!;
      const r = t.getBoundingClientRect();
      const m = document.createElement("div");
      m.setAttribute("data-asset-id", "a0");
      Object.assign(m.style, { position: "fixed", left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px`, pointerEvents: "none" });
      document.body.append(m);
      // Record the viewer's lift every frame while it opens.
      (window as unknown as { samples: number[] }).samples = [];
      const tick = () => {
        const stage = document.querySelector<HTMLElement>(".viewer-stage");
        if (stage) (window as unknown as { samples: number[] }).samples.push(new DOMMatrix(getComputedStyle(stage.parentElement!).transform).a);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    await tile.click();
    await page.getByRole("dialog", { name: "IMG_00000.jpg" }).waitFor();
    await pause(page, 700);
    const samples = await page.evaluate(() => (window as unknown as { samples: number[] }).samples);
    expect(Math.min(...samples), "it started small, at the tile's size (the first frames are already on their way)").toBeLessThan(0.8);
    expect(samples.at(-1), "and ended at full size").toBeCloseTo(1, 2);

    // Closing shrinks it back into the tile before the viewer goes.
    await page.evaluate(() => {
      (window as unknown as { samples: number[] }).samples = [];
    });
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: "IMG_00000.jpg" })).toHaveCount(0);
    const closing = await page.evaluate(() => (window as unknown as { samples: number[] }).samples);
    expect(Math.min(...closing), "it shrank into the tile").toBeLessThan(0.8);
  });
});

test.describe("less motion", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, reducedMotion: "reduce" });

  test("the photo only fades in and out: it never changes size", async ({ page }) => {
    await setupViewer(page);
    await page.goto("/library");
    const tile = page.getByRole("button", { name: /Open IMG_00000/ }).first();
    await tile.waitFor();
    await page.evaluate(() => {
      const m = document.createElement("div");
      m.setAttribute("data-asset-id", "a0");
      Object.assign(m.style, { position: "fixed", left: "10px", top: "100px", width: "90px", height: "90px", pointerEvents: "none" });
      document.body.append(m);
      (window as unknown as { samples: number[] }).samples = [];
      const tick = () => {
        const stage = document.querySelector<HTMLElement>(".viewer-stage");
        if (stage) (window as unknown as { samples: number[] }).samples.push(new DOMMatrix(getComputedStyle(stage.parentElement!).transform).a);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    await tile.click();
    await page.getByRole("dialog", { name: "IMG_00000.jpg" }).waitFor();
    await pause(page, 500);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: "IMG_00000.jpg" })).toHaveCount(0);
    const samples = await page.evaluate(() => (window as unknown as { samples: number[] }).samples);
    expect(samples.length).toBeGreaterThan(5);
    for (const s of samples) expect(s).toBeCloseTo(1, 3);
  });
});
