// Zooming the photo in the viewer, not the page: ctrl + wheel, double click, the keys and the buttons on desktop; a two-finger
// pinch (CDP touch events), one-finger pan, double tap and the swipe up for the details on a phone. The photo stays inside the
// stage when panned, the page never zooms or scrolls sideways, zoom starts over on the next photo, and the original file is
// only fetched once the zoom goes past what the preview shows.
import { expect, test, type CDPSession, type Page } from "@playwright/test";
import { openPhoto, setupViewer } from "./viewerKit";

interface Geo {
  zoom: number;
  img: { l: number; t: number; r: number; b: number; w: number; h: number };
  stage: { l: number; t: number; r: number; b: number; w: number; h: number };
  src: string;
  /** The photo's unscaled box (layout pixels), and the page's own zoom and sideways scroll. */
  layout: { w: number; h: number };
  pageScale: number;
  scrollsSideways: boolean;
  dpr: number;
}

async function geo(page: Page): Promise<Geo> {
  return page.evaluate(() => {
    const r = (el: Element) => {
      const b = el.getBoundingClientRect();
      return { l: b.left, t: b.top, r: b.right, b: b.bottom, w: b.width, h: b.height };
    };
    const img = document.querySelector<HTMLImageElement>(".lightbox-photo img")!;
    return {
      zoom: Number(document.querySelector<HTMLElement>(".lightbox-photo")!.dataset.zoom ?? "1"),
      img: r(img),
      stage: r(document.querySelector(".lightbox-stage")!),
      src: img.getAttribute("src") ?? "",
      layout: { w: img.offsetWidth, h: img.offsetHeight },
      pageScale: window.visualViewport?.scale ?? 1,
      scrollsSideways: document.documentElement.scrollWidth > window.innerWidth,
      dpr: window.devicePixelRatio,
    };
  });
}

const near = (a: number, b: number, tol = 1.5) => Math.abs(a - b) <= tol;

/** Wait for the photo to stop moving (the easing after a button, key or double tap), however slow the machine is. */
async function settled(page: Page) {
  let last = "";
  let same = 0;
  for (let i = 0; i < 100 && same < 3; i++) {
    const g = await geo(page);
    const now = JSON.stringify([g.img, g.zoom]);
    same = now === last ? same + 1 : 0;
    last = now;
    await page.waitForTimeout(40);
  }
}

/** The page itself did not zoom, move or scroll sideways. */
async function expectPageUntouched(page: Page, label: string) {
  const g = await geo(page);
  expect(g.pageScale, `${label}: page zoom`).toBe(1);
  expect(g.scrollsSideways, `${label}: sideways scroll`).toBe(false);
  expect(await page.evaluate(() => [window.scrollX, window.scrollY])).toEqual([0, 0]);
}

/** The photo covers the stage on an axis it is bigger than, and nothing of it is left off-centre on an axis it is smaller than. */
function expectCovering(g: Geo, label: string) {
  if (g.img.w >= g.stage.w) {
    expect(g.img.l, `${label}: left edge`).toBeLessThanOrEqual(g.stage.l + 1.5);
    expect(g.img.r, `${label}: right edge`).toBeGreaterThanOrEqual(g.stage.r - 1.5);
  } else {
    expect(near((g.img.l + g.img.r) / 2, (g.stage.l + g.stage.r) / 2), `${label}: centred across`).toBe(true);
  }
  if (g.img.h >= g.stage.h) {
    expect(g.img.t, `${label}: top edge`).toBeLessThanOrEqual(g.stage.t + 1.5);
    expect(g.img.b, `${label}: bottom edge`).toBeGreaterThanOrEqual(g.stage.b - 1.5);
  } else {
    expect(near((g.img.t + g.img.b) / 2, (g.stage.t + g.stage.b) / 2), `${label}: centred down`).toBe(true);
  }
}

const centreOf = (g: Geo) => ({ x: (g.stage.l + g.stage.r) / 2, y: (g.stage.t + g.stage.b) / 2 });

/** Requests of the original file, counted by the mocked bucket route. */
function originals() {
  const seen: string[] = [];
  return { seen, onOriginal: (name: string) => void seen.push(name) };
}

test.describe("zoom on desktop", () => {
  test.use({ viewport: { width: 1280, height: 720 } });

  test("ctrl + wheel zooms around the cursor, a plain wheel does nothing, the page stays put", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const g0 = await geo(page);
    expect(g0.zoom).toBe(1);
    const stageBefore = g0.stage;
    // A spot right of centre, on the photo.
    const at = { x: g0.img.l + g0.img.w * 0.75, y: g0.img.t + g0.img.h * 0.4 };
    await page.mouse.move(at.x, at.y);

    await page.mouse.wheel(0, -120);
    await page.waitForTimeout(100);
    expect((await geo(page)).zoom, "a plain wheel leaves the zoom alone").toBe(1);

    // Zoomed out the photo is centred (it cannot follow the cursor until it is bigger than the stage): zoom in with the key
    // first, then pinch with the wheel around the cursor.
    for (let i = 0; i < 3; i++) await page.keyboard.press("+");
    await settled(page);
    const gA = await geo(page);
    await page.keyboard.down("Control");
    for (let i = 0; i < 2; i++) await page.mouse.wheel(0, -20);
    await page.keyboard.up("Control");
    await expect.poll(async () => (await geo(page)).zoom).toBeGreaterThan(gA.zoom * 1.3);
    const g1 = await geo(page);
    // The photo point that was under the cursor still is.
    const fx = (at.x - gA.img.l) / gA.img.w;
    const fy = (at.y - gA.img.t) / gA.img.h;
    const msg = JSON.stringify({ at, gA: gA.img, g1: g1.img, z: g1.zoom });
    expect(near(g1.img.l + g1.img.w * fx, at.x, 2), `x under the cursor ${msg}`).toBe(true);
    expect(near(g1.img.t + g1.img.h * fy, at.y, 2), `y under the cursor ${msg}`).toBe(true);
    expect(g1.stage, "the stage did not change").toEqual(stageBefore);
    expectCovering(g1, "zoomed");
    await expectPageUntouched(page, "ctrl+wheel");

    // Out again, all the way: back at fit, centred.
    await page.keyboard.down("Control");
    for (let i = 0; i < 6; i++) await page.mouse.wheel(0, 100);
    await page.keyboard.up("Control");
    await expect.poll(async () => (await geo(page)).zoom).toBe(1);
    const g2 = await geo(page);
    expect(near(g2.img.w, g0.img.w) && near(g2.img.l, g0.img.l), "back at fit").toBe(true);
  });

  test("double click toggles between fit and a bigger zoom at the point", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const g0 = await geo(page);
    const at = { x: g0.img.l + g0.img.w * 0.8, y: g0.img.t + g0.img.h * 0.7 };
    await page.mouse.dblclick(at.x, at.y);
    await expect.poll(async () => (await geo(page)).zoom).toBeGreaterThanOrEqual(2);
    await settled(page); // the easing
    const g1 = await geo(page);
    const fx = (at.x - g0.img.l) / g0.img.w;
    const fy = (at.y - g0.img.t) / g0.img.h;
    expect(near(g1.img.l + g1.img.w * fx, at.x, 2), "x under the pointer").toBe(true);
    expect(near(g1.img.t + g1.img.h * fy, at.y, 2), "y under the pointer").toBe(true);
    expectCovering(g1, "double click zoom");
    await page.mouse.dblclick(at.x, at.y);
    await expect.poll(async () => (await geo(page)).zoom).toBe(1);
    await expectPageUntouched(page, "double click");
  });

  test("keys: + and = zoom in, - out, z between fit and zoomed; 0 to 5 still rate", async ({ page }) => {
    await setupViewer(page);
    const rated: string[] = [];
    await page.route("**/assets/rating", (r) => {
      rated.push(r.request().postData() ?? "");
      return r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ count: 1 }) });
    });
    await openPhoto(page, 0);
    const zoomOf = async () => (await geo(page)).zoom;
    await page.keyboard.press("+");
    await expect.poll(zoomOf).toBe(1.5);
    await page.keyboard.press("=");
    await expect.poll(zoomOf).toBe(2.25);
    await page.keyboard.press("-");
    await expect.poll(zoomOf).toBe(1.5);
    await page.keyboard.press("-");
    await expect.poll(zoomOf).toBe(1);
    await page.keyboard.press("-");
    expect(await zoomOf()).toBe(1);
    await page.keyboard.press("z");
    await expect.poll(zoomOf).toBeGreaterThanOrEqual(2);
    await page.keyboard.press("z");
    await expect.poll(zoomOf).toBe(1);
    // Zoom in to the maximum and stay there.
    for (let i = 0; i < 12; i++) await page.keyboard.press("+");
    await settled(page);
    const max = await zoomOf();
    expect(max).toBeGreaterThanOrEqual(4);
    expect(max).toBeLessThanOrEqual(8);
    await page.keyboard.press("+");
    expect(await zoomOf()).toBe(max);
    // Ctrl + plus is the browser's: the viewer leaves it alone.
    await page.keyboard.press("Control+=");
    expect(await zoomOf()).toBe(max);
    // The digits are still the rating.
    await page.keyboard.press("3");
    await expect.poll(() => rated.length).toBeGreaterThan(0);
  });

  test("the zoom buttons: names, level, focus and the live status", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const group = page.getByRole("group", { name: "Zoom" });
    await expect(group).toBeVisible();
    const zin = group.getByRole("button", { name: "Zoom in" });
    const zout = group.getByRole("button", { name: "Zoom out" });
    const fit = group.getByRole("button", { name: "Fit photo to screen" });
    for (const b of [zin, zout, fit]) {
      const box = (await b.boundingBox())!;
      expect(box.width).toBeGreaterThanOrEqual(36);
      expect(box.height).toBeGreaterThanOrEqual(36);
    }
    await expect(group.locator(".zoom-level")).toHaveText("100 %");
    await zin.focus();
    await zin.click();
    await expect(group.locator(".zoom-level")).toHaveText("150 %");
    await expect(zin).toBeFocused();
    await zin.click();
    await expect(group.locator(".zoom-level")).toHaveText("225 %");
    // Said aloud once it has settled.
    await expect(group.locator("[aria-live=polite]")).toHaveText("225 %");
    await fit.click();
    await expect(group.locator(".zoom-level")).toHaveText("100 %");
    await expect(fit).toBeFocused();
    await expect.poll(async () => (await geo(page)).zoom).toBe(1);
    await zout.click();
    expect((await geo(page)).zoom).toBe(1);
    // Keyboard: Tab reaches the buttons, Enter presses one.
    await zin.focus();
    await page.keyboard.press("Enter");
    await expect(group.locator(".zoom-level")).toHaveText("150 %");
  });

  test("drag pans a zoomed photo but never off the stage; at fit a drag does nothing", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const g0 = await geo(page);
    const c = centreOf(g0);
    // At fit: nothing to pan.
    await page.mouse.move(c.x, c.y);
    await page.mouse.down();
    await page.mouse.move(c.x + 120, c.y + 60, { steps: 4 });
    await page.mouse.up();
    expect((await geo(page)).img.l).toBeCloseTo(g0.img.l, 0);

    for (let i = 0; i < 4; i++) await page.keyboard.press("+");
    await settled(page);
    const g1 = await geo(page);
    expect(g1.zoom).toBeGreaterThan(4);
    expectCovering(g1, "zoomed");
    await page.mouse.move(c.x, c.y);
    await page.mouse.down();
    await page.mouse.move(c.x - 150, c.y - 80, { steps: 6 });
    await page.mouse.up();
    const g2 = await geo(page);
    expect(near(g2.img.l, g1.img.l - 150, 2), "moved with the drag").toBe(true);
    expect(near(g2.img.t, g1.img.t - 80, 2)).toBe(true);
    expectCovering(g2, "panned");
    // Thrown far past the edge: it stops at the edge.
    await page.mouse.move(c.x, c.y);
    await page.mouse.down();
    await page.mouse.move(c.x + 5000, c.y + 5000, { steps: 8 });
    await page.mouse.up();
    const g3 = await geo(page);
    expect(near(g3.img.l, g3.stage.l, 1.5), "stopped at the left edge of the stage").toBe(true);
    expect(near(g3.img.t, g3.stage.t, 1.5), "stopped at the top").toBe(true);
    expectCovering(g3, "thrown right");
    await page.mouse.move(c.x, c.y);
    await page.mouse.down();
    await page.mouse.move(c.x - 9000, c.y - 9000, { steps: 8 });
    await page.mouse.up();
    const g4 = await geo(page);
    expect(near(g4.img.r, g4.stage.r, 1.5) && near(g4.img.b, g4.stage.b, 1.5), "stopped at the far corner").toBe(true);
    expectCovering(g4, "thrown left");
    await expectPageUntouched(page, "pan");
  });

  test("a panorama and a portrait pan only along the side that overflows", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await setupViewer(page);
    await openPhoto(page, 2); // the panorama: fits the width, short
    await page.keyboard.press("+");
    await settled(page);
    const g = await geo(page);
    expect(g.img.h, "1.5x of the panorama is still shorter than the stage").toBeLessThan(g.stage.h);
    expect(g.img.w, "and wider").toBeGreaterThan(g.stage.w);
    expectCovering(g, "panorama 1.5x");
    const c = centreOf(g);
    await page.mouse.move(c.x, c.y);
    await page.mouse.down();
    await page.mouse.move(c.x - 100, c.y + 200, { steps: 5 });
    await page.mouse.up();
    const after = await geo(page);
    expect(near(after.img.t, g.img.t, 1), "no vertical room, no vertical move").toBe(true);
    expectCovering(after, "panorama panned");
  });

  test("zoom starts over on the next photo and when the viewer reopens", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    await page.keyboard.press("+");
    await page.keyboard.press("+");
    await expect.poll(async () => (await geo(page)).zoom).toBe(2.25);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", /portrait\.svg/);
    expect((await geo(page)).zoom).toBe(1);
    await page.keyboard.press("ArrowLeft");
    await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", /landscape\.svg/);
    expect((await geo(page)).zoom).toBe(1);
    await page.keyboard.press("+");
    await page.getByRole("dialog").getByRole("button", { name: "Close" }).click();
    await page.getByRole("button", { name: /Open IMG_00000/ }).first().click();
    await page.locator(".lightbox-stage img.loaded").waitFor();
    expect((await geo(page)).zoom).toBe(1);
  });

  test("the info panel opening re-fits a zoomed photo inside the narrower stage", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    for (let i = 0; i < 4; i++) await page.keyboard.press("+");
    await settled(page);
    await page.keyboard.press("i");
    await expect(page.locator("#details-panel")).toBeVisible();
    await settled(page);
    expectCovering(await geo(page), "zoomed with the panel open");
  });

  test("the sharper picture is requested only past the preview's resolution, and swaps without a jump", async ({ page }) => {
    const o = originals();
    await setupViewer(page, { onOriginal: o.onOriginal });
    await openPhoto(page, 0);
    const g0 = await geo(page);
    const fitLong = Math.max(g0.layout.w, g0.layout.h);
    const previewLong = 1440;
    // The zoom at which the drawn photo has as many device pixels as the preview.
    const threshold = previewLong / (fitLong * g0.dpr);
    expect(threshold, "the test photo needs room to zoom below the threshold").toBeGreaterThan(1.6);
    await page.waitForTimeout(200);
    expect(o.seen, "nothing at fit").toEqual([]);

    await page.keyboard.press("+"); // 1.5
    await page.waitForTimeout(400);
    expect(1.5, "the key's first step is below the threshold").toBeLessThan(threshold);
    expect(o.seen, "below the threshold").toEqual([]);
    expect((await geo(page)).src).toMatch(/fixture-preview/);

    const before = await geo(page);
    for (let i = 0; i < 6 && (await geo(page)).zoom <= threshold; i++) await page.keyboard.press("+");
    await expect.poll(() => o.seen.length, { message: "requested past the threshold" }).toBeGreaterThan(0);
    await expect.poll(async () => (await geo(page)).src).toMatch(/fixture-original/);
    await settled(page);
    const after = await geo(page);
    // Same layout box: the swap did not resize or move the photo's box.
    expect(after.layout).toEqual(g0.layout);
    expect(before.layout).toEqual(g0.layout);
    expectCovering(after, "after the swap");
    expect(new Set(o.seen).size, "one picture, fetched for this photo only").toBe(1);
    // Going back out keeps the sharp one (no flicker); the next photo starts with its preview.
    await page.keyboard.press("0");
    await page.keyboard.press("ArrowRight");
    await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", /fixture-preview\/portrait/);
  });

  test("no sharper picture for an edited photo, HEIC or a file that is not larger", async ({ page }) => {
    const o = originals();
    await setupViewer(page, { onOriginal: o.onOriginal, edited: ["a0"] });
    await openPhoto(page, 0);
    for (let i = 0; i < 8; i++) await page.keyboard.press("+");
    await page.waitForTimeout(600);
    expect((await geo(page)).zoom).toBeGreaterThan(4);
    expect(o.seen, "edited: the preview is the picture").toEqual([]);
    expect((await geo(page)).src).toMatch(/fixture-preview/);
  });

  test("a HEIC original is never shown", async ({ page }) => {
    const o = originals();
    await setupViewer(page, { onOriginal: o.onOriginal, contentType: "image/heic" });
    await openPhoto(page, 0);
    for (let i = 0; i < 8; i++) await page.keyboard.press("+");
    await page.waitForTimeout(600);
    expect(o.seen).toEqual([]);
  });

  test("a file no larger than the preview has no sharper picture", async ({ page }) => {
    const o = originals();
    await setupViewer(page, { onOriginal: o.onOriginal, originalScale: 1 });
    await openPhoto(page, 0);
    for (let i = 0; i < 8; i++) await page.keyboard.press("+");
    await page.waitForTimeout(600);
    expect(o.seen).toEqual([]);
  });

  test("the app's own viewport still allows page zoom, and only the photo area takes touch gestures", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const meta = await page.evaluate(() => document.querySelector('meta[name="viewport"]')?.getAttribute("content") ?? "");
    expect(meta).not.toMatch(/user-scalable|maximum-scale|minimum-scale/i);
    const actions = await page.evaluate(() => {
      const ta = (sel: string) => getComputedStyle(document.querySelector(sel)!).touchAction;
      return { stage: ta(".lightbox-stage"), bar: ta(".lightbox-bar"), lightbox: ta(".lightbox"), body: ta("body"), html: ta("html") };
    });
    expect(actions.stage).toBe("none");
    expect(actions.bar).toBe("auto");
    expect(actions.lightbox).toBe("auto");
    expect(actions.body).toBe("auto");
    expect(actions.html).toBe("auto");
  });
});

// ---- Touch ---------------------------------------------------------------------------------------------------------

type Pt = { x: number; y: number };

async function fingers(cdp: CDPSession, type: "touchStart" | "touchMove" | "touchEnd", pts: Pt[]) {
  await cdp.send("Input.dispatchTouchEvent", { type, touchPoints: pts.map((p, id) => ({ x: p.x, y: p.y, id })) });
}

/** Two fingers from `a` to `b` (each a pair of points), in steps. */
async function pinch(cdp: CDPSession, a: [Pt, Pt], b: [Pt, Pt], steps = 8) {
  await fingers(cdp, "touchStart", a);
  for (let i = 1; i <= steps; i++) {
    const t = i / steps;
    await fingers(
      cdp,
      "touchMove",
      a.map((p, k) => ({ x: p.x + (b[k]!.x - p.x) * t, y: p.y + (b[k]!.y - p.y) * t })),
    );
  }
  await fingers(cdp, "touchEnd", []);
}

/** One finger from `a` to `b`, in `steps` moves. */
async function drag(cdp: CDPSession, a: Pt, b: Pt, steps = 8) {
  await fingers(cdp, "touchStart", [a]);
  for (let i = 1; i <= steps; i++) {
    const t = i / steps;
    await fingers(cdp, "touchMove", [{ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t }]);
  }
  await fingers(cdp, "touchEnd", []);
}

/** `times` quick taps sent back to back (the commands are queued before the first answer, so a slow machine cannot space them out). */
async function tap(cdp: CDPSession, p: Pt, times = 1) {
  const sent: Promise<unknown>[] = [];
  for (let i = 0; i < times; i++) {
    sent.push(fingers(cdp, "touchStart", [p]));
    sent.push(fingers(cdp, "touchEnd", []));
  }
  await Promise.all(sent);
}

test.describe("zoom on a phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });

  test("pinch zooms the photo around the fingers, pan is clamped, the page does not zoom", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const cdp = await page.context().newCDPSession(page);
    const g0 = await geo(page);
    const c = centreOf(g0);

    // Spread two fingers around a point right of centre.
    const mid = { x: c.x + 60, y: c.y - 20 };
    await pinch(cdp, [{ x: mid.x - 20, y: mid.y }, { x: mid.x + 20, y: mid.y }], [{ x: mid.x - 70, y: mid.y }, { x: mid.x + 70, y: mid.y }]);
    const g1 = await geo(page);
    expect(g1.zoom, "3.5x for a finger distance of 40 to 140").toBeGreaterThan(3);
    expect(g1.zoom).toBeLessThanOrEqual(8);
    const fx = (mid.x - g0.img.l) / g0.img.w;
    const fy = (mid.y - g0.img.t) / g0.img.h;
    expect(near(g1.img.l + g1.img.w * fx, mid.x, 3), "the photo point between the fingers stays between them (x)").toBe(true);
    expect(near(g1.img.t + g1.img.h * fy, mid.y, 3), "(y)").toBe(true);
    expectCovering(g1, "pinched");
    expect(g1.pageScale, "the page was not zoomed").toBe(1);
    expect(g1.scrollsSideways).toBe(false);
    await expect(page.getByRole("group", { name: "Zoom" })).toBeVisible(); // shown while zoomed
    await expect(page.locator(".zoom-level")).toHaveText(`${Math.round(g1.zoom * 100)} %`);

    // One finger pans, and thrown past the edge it stops there.
    await drag(cdp, { x: c.x, y: c.y }, { x: c.x - 60, y: c.y + 40 });
    const g2 = await geo(page);
    expect(near(g2.img.l, g1.img.l - 60, 3) && near(g2.img.t, g1.img.t + 40, 3), "panned with the finger").toBe(true);
    expectCovering(g2, "panned");
    await drag(cdp, { x: 20, y: g2.stage.t + 40 }, { x: 380, y: g2.stage.b - 40 }, 6);
    await drag(cdp, { x: 20, y: g2.stage.t + 40 }, { x: 380, y: g2.stage.b - 40 }, 6);
    await drag(cdp, { x: 20, y: g2.stage.t + 40 }, { x: 380, y: g2.stage.b - 40 }, 6);
    const g3 = await geo(page);
    expect(near(g3.img.l, g3.stage.l, 1.5) && near(g3.img.t, g3.stage.t, 1.5), "stopped at the stage's top left").toBe(true);
    expectCovering(g3, "thrown");
    await expectPageUntouched(page, "pan");

    // Pinch back in: snaps to fit, centred.
    // (Away from the arrows at the sides: a finger on one of those is a press of the button, not part of a pinch.)
    await pinch(cdp, [{ x: 40, y: c.y - 150 }, { x: 350, y: c.y - 150 }], [{ x: 190, y: c.y - 150 }, { x: 200, y: c.y - 150 }], 8);
    await expect.poll(async () => (await geo(page)).zoom).toBe(1);
    await settled(page);
    const g4 = await geo(page);
    expect(near(g4.img.l, g0.img.l) && near(g4.img.t, g0.img.t) && near(g4.img.w, g0.img.w), "back at fit").toBe(true);
    await expectPageUntouched(page, "pinch out");
  });

  test("double tap toggles fit and zoom at the finger", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 1); // portrait
    const cdp = await page.context().newCDPSession(page);
    const g0 = await geo(page);
    const at = { x: g0.img.l + g0.img.w * 0.3, y: g0.img.t + g0.img.h * 0.3 };
    await tap(cdp, at, 2);
    await expect.poll(async () => (await geo(page)).zoom).toBeGreaterThanOrEqual(2);
    await settled(page);
    const g1 = await geo(page);
    const fx = (at.x - g0.img.l) / g0.img.w;
    const fy = (at.y - g0.img.t) / g0.img.h;
    expect(near(g1.img.l + g1.img.w * fx, at.x, 3) && near(g1.img.t + g1.img.h * fy, at.y, 3), "zoomed at the tap").toBe(true);
    await tap(cdp, at, 2);
    await expect.poll(async () => (await geo(page)).zoom).toBe(1);
    // A single tap does nothing.
    await tap(cdp, at);
    await page.waitForTimeout(500);
    expect((await geo(page)).zoom).toBe(1);
    await expectPageUntouched(page, "double tap");
  });

  test("swipe up opens the details at fit, and is off while zoomed (the finger pans instead)", async ({ page }) => {
    await setupViewer(page);
    await openPhoto(page, 0);
    const cdp = await page.context().newCDPSession(page);
    const g0 = await geo(page);
    const c = centreOf(g0);
    // Zoomed: a long swipe up pans (or does nothing at the edge) and never opens the panel.
    await page.keyboard.press("+");
    await page.keyboard.press("+");
    await page.keyboard.press("+");
    await settled(page);
    await drag(cdp, { x: c.x, y: c.y + 150 }, { x: c.x, y: c.y - 150 }, 6);
    await settled(page);
    await expect(page.locator("#details-panel")).toHaveCount(0);
    expect((await geo(page)).zoom).toBeGreaterThan(2);
    // Same index, still: no paging either.
    await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", /landscape\.svg/);
    // Back at fit the swipe works again.
    await page.keyboard.press("z");
    await expect.poll(async () => (await geo(page)).zoom).toBe(1);
    await settled(page);
    await drag(cdp, { x: c.x, y: c.y + 150 }, { x: c.x, y: c.y - 150 }, 6);
    await expect(page.locator("#details-panel")).toBeVisible();
  });

  test("the sharper picture is requested past the threshold on a 2x screen", async ({ page }) => {
    const o = originals();
    await setupViewer(page, { onOriginal: o.onOriginal });
    await openPhoto(page, 0);
    const cdp = await page.context().newCDPSession(page);
    const g0 = await geo(page);
    const threshold = 1440 / (Math.max(g0.layout.w, g0.layout.h) * g0.dpr);
    expect(g0.dpr).toBe(2);
    const c = centreOf(g0);
    await pinch(cdp, [{ x: c.x - 20, y: c.y }, { x: c.x + 20, y: c.y }], [{ x: c.x - 20 * threshold * 0.7, y: c.y }, { x: c.x + 20 * threshold * 0.7, y: c.y }], 6);
    const below = await geo(page);
    expect(below.zoom).toBeLessThan(threshold);
    expect(below.zoom).toBeGreaterThan(1.05);
    await page.waitForTimeout(400);
    expect(o.seen, "below the threshold: preview only").toEqual([]);
    await pinch(cdp, [{ x: c.x - 40, y: c.y }, { x: c.x + 40, y: c.y }], [{ x: c.x - 40 * 3, y: c.y }, { x: c.x + 40 * 3, y: c.y }], 6);
    await expect.poll(() => o.seen.length).toBeGreaterThan(0);
    await expect.poll(async () => (await geo(page)).src).toMatch(/fixture-original/);
  });
});

test.describe("zoom on the share page", () => {
  test.use({ viewport: { width: 1280, height: 720 } });

  test("the public viewer zooms the preview with the same controls", async ({ page }) => {
    const { SHAPES, installPictures } = await import("./viewerKit");
    await installPictures(page, 3);
    const items = SHAPES.map((s, i) => ({
      id: `p${i}`,
      width: s.w,
      height: s.h,
      sort_time: "2026-10-14T10:00:00Z",
      thumbhash: null,
      thumb_url: `/fixture-preview/${s.name}.svg`,
      preview_url: `/fixture-preview/${s.name}.svg`,
      downloads: [],
      download_pending: false,
    }));
    await page.route("**/assets/public/s/open", (r) =>
      r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ needs_password: false, unlocked: true, kind: "selection", title: null, item_count: items.length, allow_download: false, include_originals: false, expires_at: null }) }),
    );
    await page.route(/\/assets\/public\/s\/open\/items(\?.*)?$/, (r) =>
      r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items, next_cursor: null }) }),
    );
    await page.goto("/s/open");
    await page.getByRole("button", { name: /Open photo 1 of/ }).click();
    await page.locator(".lightbox-stage img.loaded").waitFor();
    await page.getByRole("button", { name: "Zoom in" }).click();
    await expect(page.locator(".zoom-level")).toHaveText("150 %");
    expect((await geo(page)).zoom).toBe(1.5);
    await page.keyboard.press("+");
    await expect.poll(async () => (await geo(page)).zoom).toBe(2.25);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", /portrait\.svg/);
    expect((await geo(page)).zoom).toBe(1);
    await page.mouse.dblclick(640, 400);
    await expect.poll(async () => (await geo(page)).zoom).toBeGreaterThanOrEqual(2);
    expectCovering(await geo(page), "share zoomed");
  });
});
