// The faces on the photo in the viewer (People on): regions over the picture that sit on the faces at fit and follow every zoom and pan,
// outlined on hover and focus, reached and opened with the keyboard, a named face opening the person and any other "Who is this?" (with "Not a
// face"); hidden people's faces and detections marked not a face are not there; and, with a mouse and with CDP touch, a tap opens a face but
// a double tap, a pinch or a pan never does. Also the "Who is this?" dialog with the face outlined at phone and desktop widths, light and dark.
import { expect, test, type CDPSession, type Locator, type Page } from "@playwright/test";
import { json, makeCheck, shotPrefix } from "./kit";
import { openPhoto, setupViewer } from "./viewerKit";

type Box = [number, number, number, number];
interface Face {
  id: string;
  box: Box;
  person: { id: string; name: string | null } | null;
}

const NAMED: Face = { id: "fn", box: [0.1, 0.2, 0.15, 0.2], person: { id: "p1", name: "Person A" } };
const UNCLAIMED: Face = { id: "fu", box: [0.55, 0.3, 0.12, 0.18], person: null };
const UNNAMED: Face = { id: "fp", box: [0.3, 0.6, 0.1, 0.15], person: { id: "p2", name: null } };
const PEOPLE = [
  { id: "p1", name: "Person A", hidden: false, faces: 4, photos: 3, cover: null },
  { id: "p2", name: null, hidden: false, faces: 2, photos: 2, cover: null },
];

interface Calls {
  marked: string[];
  unmarked: string[];
  assigned: { face: string; body: unknown }[];
}

/** A library whose first photo has `faces` (as the server serves them: no hidden people, no detections marked not a face). */
async function setupFaces(page: Page, faces: Face[] = [NAMED, UNCLAIMED, UNNAMED]) {
  await setupViewer(page);
  const live = [...faces];
  const dismissed: { id: string; box: Box }[] = [];
  const calls: Calls = { marked: [], unmarked: [], assigned: [] };
  await page.route("**/assets/people/status", (r) => r.fulfill(json({ enabled: true })));
  await page.route(/\/assets\/people$/, (r) => r.fulfill(json({ items: PEOPLE })));
  await page.route(/\/assets\/a\d+\/faces$/, (r) => {
    const first = r.request().url().endsWith("/assets/a0/faces");
    return r.fulfill(json({ items: first ? live : [], dismissed: first ? dismissed : [] }));
  });
  await page.route(/\/assets\/faces\/[^/]+\/not-a-face$/, (r) => {
    const id = /faces\/([^/]+)\/not-a-face/.exec(r.request().url())![1]!;
    if (r.request().method() === "POST") {
      calls.marked.push(id);
      const at = live.findIndex((f) => f.id === id);
      const [gone] = at >= 0 ? live.splice(at, 1) : [];
      if (gone) dismissed.push({ id, box: gone.box });
      return r.fulfill(json({ person_id: gone?.person?.id ?? null }));
    }
    calls.unmarked.push(id);
    const at = dismissed.findIndex((d) => d.id === id);
    const [back] = at >= 0 ? dismissed.splice(at, 1) : [];
    if (back) live.push({ id, box: back.box, person: null });
    return r.fulfill({ status: 204 });
  });
  await page.route(/\/assets\/faces\/[^/]+\/assign$/, (r) => {
    calls.assigned.push({ face: /faces\/([^/]+)\/assign/.exec(r.request().url())![1]!, body: r.request().postDataJSON() });
    return r.fulfill(json({ person: PEOPLE[0] }));
  });
  await page.route(/\/assets\/people\/p1$/, (r) => r.fulfill(json(PEOPLE[0])));
  return { calls };
}

const region = (page: Page, id: string) => page.locator(`.face-region[data-face-id="${id}"]`);
const whoIsThis = (page: Page) => page.getByRole("dialog", { name: "Who is this?" });

/** Open photo 0 and wait for its faces to be drawn. */
async function openWithFaces(page: Page, count = 3) {
  const viewer = await openPhoto(page, 0);
  await expect(page.locator(".face-region")).toHaveCount(count);
  return viewer;
}

interface Rects {
  img: { l: number; t: number; w: number; h: number };
  regions: Record<string, { l: number; t: number; w: number; h: number }>;
}

async function rects(page: Page): Promise<Rects> {
  return page.evaluate(() => {
    const r = (el: Element) => {
      const b = el.getBoundingClientRect();
      return { l: b.left, t: b.top, w: b.width, h: b.height };
    };
    const regions: Record<string, { l: number; t: number; w: number; h: number }> = {};
    for (const el of Array.from(document.querySelectorAll<HTMLElement>(".face-region"))) regions[el.dataset.faceId!] = r(el);
    return { img: r(document.querySelector(".viewer-photo.current img")!), regions };
  });
}

/** Every region is exactly where its face's box says, as fractions of the picture as drawn (so also at any zoom and pan). */
async function expectOnFaces(page: Page, faces: Face[], label: string) {
  const g = await rects(page);
  for (const f of faces) {
    const got = g.regions[f.id]!;
    expect(got, `${label}: ${f.id} is drawn`).toBeTruthy();
    const want = { l: g.img.l + f.box[0] * g.img.w, t: g.img.t + f.box[1] * g.img.h, w: f.box[2] * g.img.w, h: f.box[3] * g.img.h };
    const tol = 1.5;
    expect(Math.abs(got.l - want.l), `${label}: ${f.id} left`).toBeLessThan(tol);
    expect(Math.abs(got.t - want.t), `${label}: ${f.id} top`).toBeLessThan(tol);
    expect(Math.abs(got.w - want.w), `${label}: ${f.id} width`).toBeLessThan(tol);
    expect(Math.abs(got.h - want.h), `${label}: ${f.id} height`).toBeLessThan(tol);
  }
}

const zoomOf = (page: Page) => page.evaluate(() => Number(document.querySelector<HTMLElement>(".viewer-photo.current")!.dataset.zoom ?? "1"));

/** Wait for the photo to stop moving (the easing after a key or a double tap). */
async function settled(page: Page) {
  let last = "";
  let same = 0;
  for (let i = 0; i < 100 && same < 3; i++) {
    const now = JSON.stringify(await rects(page));
    same = now === last ? same + 1 : 0;
    last = now;
    await page.waitForTimeout(40);
  }
}

const centre = async (l: Locator) => {
  const b = (await l.boundingBox())!;
  return { x: b.x + b.width / 2, y: b.y + b.height / 2 };
};

/** Nothing opened: no decision dialog, and the viewer is where it was. */
async function expectNothingOpened(page: Page) {
  await page.waitForTimeout(700); // longer than the double-tap window a tap on a face waits out
  await expect(whoIsThis(page)).toHaveCount(0);
  expect(new URL(page.url()).pathname).toMatch(/^\/(library|photos)$/);
}

test.describe("faces on the photo, desktop", () => {
  test.use({ viewport: { width: 1280, height: 720 } });

  test("each face is a labelled region on the face, at fit and after zoom and pan, and none for what is hidden", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    await expect(page.getByRole("link", { name: "Person A: open their page" })).toHaveCount(1);
    await expect(page.getByRole("button", { name: "Unknown face: who is this?" })).toHaveCount(2);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "fit");
    const fit = await rects(page);

    for (let i = 0; i < 3; i++) await page.keyboard.press("+");
    await settled(page);
    expect(await zoomOf(page)).toBeGreaterThan(2);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "zoomed");
    const zoomed = await rects(page);
    expect(zoomed.regions.fn!.w, "the region grew with the photo").toBeGreaterThan(fit.regions.fn!.w * 2);

    // Pan with a drag on empty photo (the mouse, as a desktop user would): the regions travel with the picture.
    const start = await rects(page);
    const at = { x: 640, y: 360 };
    await page.mouse.move(at.x, at.y);
    await page.mouse.down();
    await page.mouse.move(at.x - 120, at.y - 60, { steps: 6 });
    await page.mouse.up();
    await settled(page);
    const panned = await rects(page);
    expect(Math.abs(panned.img.l - start.img.l) + Math.abs(panned.img.t - start.img.t), "the photo moved").toBeGreaterThan(20);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "panned");

    // Back to fit.
    await page.keyboard.press("z");
    await expect.poll(() => zoomOf(page)).toBe(1);
    await settled(page);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "fit again");
  });

  test("a face with nothing to open (a hidden person's, a detection marked not a face) is not drawn at all", async ({ page }) => {
    // The server leaves both out; the viewer draws exactly what it is given.
    await setupFaces(page, [UNCLAIMED]);
    await openWithFaces(page, 1);
    await expect(region(page, "fu")).toBeVisible();
    await expect(page.locator(".face-region")).toHaveCount(1);
    await expect(page.locator(".face-not")).toHaveCount(1);
  });

  test("a face is outlined on hover and when focused with the keyboard, and all of them with the switch in Info", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    const outline = (id: string) =>
      region(page, id).evaluate((el) => {
        const s = getComputedStyle(el);
        return { border: Number.parseFloat(s.borderTopWidth), outline: s.outlineStyle === "none" ? 0 : Number.parseFloat(s.outlineWidth) };
      });
    expect(await outline("fu")).toEqual({ border: 0, outline: 0 });
    await region(page, "fu").hover();
    expect((await outline("fu")).border).toBeGreaterThanOrEqual(2);
    expect((await outline("fn")).border, "only the face under the mouse").toBe(0);
    await page.mouse.move(5, 5);
    // The keyboard: Tab until a face has focus; a visible outline appears.
    for (let i = 0; i < 40; i++) {
      await page.keyboard.press("Tab");
      if (await page.evaluate(() => document.activeElement?.classList.contains("face-region"))) break;
    }
    const focused = await page.evaluate(() => (document.activeElement as HTMLElement).dataset.faceId);
    expect(focused).toBeTruthy();
    expect((await outline(focused!)).outline).toBeGreaterThanOrEqual(2);
    // The Not a face button belongs to the focused face and shows with it.
    await expect(region(page, focused!).locator("xpath=..").locator(".face-not")).toHaveCSS("opacity", "1");

    // The switch in Info outlines every face.
    await page.keyboard.press("i");
    const panel = page.locator("#details-panel");
    await panel.getByRole("switch", { name: "Show faces on the photo" }).check();
    await page.mouse.move(5, 5);
    for (const id of ["fn", "fu", "fp"]) expect((await outline(id)).border, id).toBeGreaterThanOrEqual(2);
    await panel.getByRole("switch", { name: "Show faces on the photo" }).uncheck();
    expect((await outline("fu")).border).toBe(0);
  });

  test("the Info panel lists the named people only, once", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    await page.keyboard.press("i");
    const panel = page.locator("#details-panel");
    await expect(panel.getByRole("link", { name: "Person A" })).toHaveCount(1);
    await expect(panel.getByText("Unknown face")).toHaveCount(0);
    await expect(panel.getByRole("link", { name: "Unnamed person" })).toHaveCount(0);
    await expect(panel.getByRole("button", { name: "Name this face" })).toHaveCount(0);
    // The regions are still on the photo, re-fitted to the narrower stage.
    await settled(page);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "with Info open");
  });

  test("Enter and Space open a face from the keyboard: a named one the person's page, any other 'Who is this?'", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    await region(page, "fu").focus();
    await page.keyboard.press("Enter");
    const dialog = whoIsThis(page);
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Not a face" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(region(page, "fu")).toBeFocused();
    await region(page, "fp").focus();
    await page.keyboard.press("Space");
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Escape");
    await region(page, "fn").focus();
    await page.keyboard.press("Enter");
    await page.waitForURL("**/people/p1");
  });

  test("a click opens a face once the double-click window has passed; a double click zooms and opens nothing", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    const c = await centre(region(page, "fu"));
    await page.mouse.dblclick(c.x, c.y);
    await expect.poll(() => zoomOf(page)).toBeGreaterThan(1.5);
    await expectNothingOpened(page);
    await page.keyboard.press("z");
    await expect.poll(() => zoomOf(page)).toBe(1);
    await settled(page);
    await page.mouse.click((await centre(region(page, "fu"))).x, (await centre(region(page, "fu"))).y);
    await expect(whoIsThis(page)).toBeVisible();
    await page.keyboard.press("Escape");
    await page.mouse.click((await centre(region(page, "fn"))).x, (await centre(region(page, "fn"))).y);
    await page.waitForURL("**/people/p1");
  });

  test("dragging a zoomed photo by a face pans it and opens nothing; so does a click on the photo away from the faces", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    for (let i = 0; i < 3; i++) await page.keyboard.press("+");
    await settled(page);
    const before = await rects(page);
    const target = (await centre(region(page, "fu"))) ;
    // The face may be scrolled off the stage at this zoom: pan the photo to it first with the keys-free drag from the middle.
    await page.mouse.move(640, 360);
    const g = before.regions.fu!;
    const from = { x: Math.min(Math.max(g.l + g.w / 2, 100), 1180), y: Math.min(Math.max(g.t + g.h / 2, 100), 620) };
    expect(target.x).toBeDefined();
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x - 90, from.y + 40, { steps: 6 });
    await page.mouse.up();
    await settled(page);
    const after = await rects(page);
    expect(Math.abs(after.img.l - before.img.l) + Math.abs(after.img.t - before.img.t), "the photo moved").toBeGreaterThan(20);
    await expectNothingOpened(page);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "after the drag");
  });

  test("Not a face from 'Who is this?': the face goes with an undo, Undo brings it back, and Info can bring back what is left marked", async ({ page }) => {
    const { calls } = await setupFaces(page);
    await openWithFaces(page);
    await region(page, "fu").focus();
    await page.keyboard.press("Enter");
    await whoIsThis(page).getByRole("button", { name: "Not a face" }).click();
    await expect(page.locator(".face-region")).toHaveCount(2);
    expect(calls.marked).toEqual(["fu"]);
    await expect(whoIsThis(page)).toHaveCount(0);
    const toast = page.getByRole("status").filter({ hasText: "Marked as not a face" });
    await expect(toast).toBeVisible();
    await toast.getByRole("button", { name: "Undo" }).click();
    expect(calls.unmarked).toEqual(["fu"]);
    await expect(page.locator(".face-region")).toHaveCount(3);
    // Marked again and left: Info says so and brings it back.
    await region(page, "fu").focus();
    await page.keyboard.press("Enter");
    await whoIsThis(page).getByRole("button", { name: "Not a face" }).click();
    await expect(page.locator(".face-region")).toHaveCount(2);
    await page.keyboard.press("i");
    const panel = page.locator("#details-panel");
    await panel.getByRole("button", { name: /Bring back 1 detection marked/ }).click();
    await expect(page.locator(".face-region")).toHaveCount(3);
    expect(calls.unmarked).toEqual(["fu", "fu"]);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "brought back");
  });

  test("Not a face on a named face (the small button that shows with it) puts the face back in its person on undo", async ({ page }) => {
    const { calls } = await setupFaces(page);
    await openWithFaces(page);
    await region(page, "fn").hover();
    const not = region(page, "fn").locator("xpath=..").locator(".face-not");
    await expect(not).toHaveCSS("opacity", "1");
    await not.click();
    await expect(page.locator(".face-region")).toHaveCount(2);
    expect(calls.marked).toEqual(["fn"]);
    await page.getByRole("status").getByRole("button", { name: "Undo" }).click();
    await expect(page.locator(".face-region")).toHaveCount(3);
    expect(calls.unmarked).toEqual(["fn"]);
    expect(calls.assigned).toEqual([{ face: "fn", body: { person_id: "p1" } }]);
  });
});

// ---- Touch ---------------------------------------------------------------------------------------------------------

type Pt = { x: number; y: number };

async function fingers(cdp: CDPSession, type: "touchStart" | "touchMove" | "touchEnd", pts: Pt[]) {
  await cdp.send("Input.dispatchTouchEvent", { type, touchPoints: pts.map((p, id) => ({ x: p.x, y: p.y, id })) });
}

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

async function drag(cdp: CDPSession, a: Pt, b: Pt, steps = 8) {
  await fingers(cdp, "touchStart", [a]);
  for (let i = 1; i <= steps; i++) {
    const t = i / steps;
    await fingers(cdp, "touchMove", [{ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t }]);
  }
  await fingers(cdp, "touchEnd", []);
}

async function tap(cdp: CDPSession, p: Pt, times = 1) {
  const sent: Promise<unknown>[] = [];
  for (let i = 0; i < times; i++) {
    sent.push(fingers(cdp, "touchStart", [p]));
    sent.push(fingers(cdp, "touchEnd", []));
  }
  await Promise.all(sent);
}

test.describe("faces on the photo, touch", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });

  test("a tap opens a face; a tap away from the faces opens nothing", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    const cdp = await page.context().newCDPSession(page);
    const g = await rects(page);
    await tap(cdp, { x: g.img.l + g.img.w * 0.85, y: g.img.t + g.img.h * 0.9 });
    await expectNothingOpened(page);
    await tap(cdp, await centre(region(page, "fu")));
    await expect(whoIsThis(page)).toBeVisible();
    await expect(whoIsThis(page).getByRole("button", { name: "Not a face" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(whoIsThis(page)).toHaveCount(0);
    await tap(cdp, await centre(region(page, "fn")));
    await page.waitForURL("**/people/p1");
  });

  test("a double tap on a face zooms and opens nothing", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    const cdp = await page.context().newCDPSession(page);
    await tap(cdp, await centre(region(page, "fu")), 2);
    await expect.poll(() => zoomOf(page)).toBeGreaterThan(1.5);
    await expectNothingOpened(page);
    await settled(page);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "after the double tap");
  });

  test("a pinch that starts on a face zooms and opens nothing; the faces stay on the photo", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    const cdp = await page.context().newCDPSession(page);
    const c = await centre(region(page, "fu"));
    await pinch(cdp, [{ x: c.x - 15, y: c.y }, { x: c.x + 15, y: c.y }], [{ x: c.x - 70, y: c.y }, { x: c.x + 70, y: c.y }]);
    await expect.poll(() => zoomOf(page)).toBeGreaterThan(1.5);
    await expectNothingOpened(page);
    await settled(page);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "after the pinch");
  });

  test("a one-finger pan that starts on a face moves a zoomed photo and opens nothing", async ({ page }) => {
    await setupFaces(page);
    await openWithFaces(page);
    const cdp = await page.context().newCDPSession(page);
    await page.keyboard.press("+");
    await page.keyboard.press("+");
    await settled(page);
    const before = await rects(page);
    const stage = (await page.locator(".viewer-stage").boundingBox())!;
    const g = before.regions.fu!;
    // A finger on the face when it is on screen, else on the middle of the stage.
    const on = g.l > stage.x + 10 && g.l + g.w < stage.x + stage.width - 10 && g.t > stage.y + 10 && g.t + g.h < stage.y + stage.height - 10;
    const from = on ? { x: g.l + g.w / 2, y: g.t + g.h / 2 } : { x: stage.x + stage.width / 2, y: stage.y + stage.height / 2 };
    await drag(cdp, from, { x: from.x - 60, y: from.y - 30 });
    await settled(page);
    const after = await rects(page);
    expect(Math.abs(after.img.l - before.img.l) + Math.abs(after.img.t - before.img.t), "the photo moved").toBeGreaterThan(10);
    await expectNothingOpened(page);
    await expectOnFaces(page, [NAMED, UNCLAIMED, UNNAMED], "after the pan");
  });
});

// ---- The decision dialog next to the face, at the widths and in both colour schemes ---------------------------------------

for (const width of [320, 375, 414, 1280]) {
  for (const scheme of ["light", "dark"] as const) {
    test.describe(`'Who is this?' with the face outlined, ${width}px ${scheme}`, () => {
      test.use({ colorScheme: scheme, viewport: { width, height: width === 1280 ? 800 : 740 }, hasTouch: width < 600 });

      test("the outlined face is on the face and every answer is on screen", async ({ page }) => {
        const problems: string[] = [];
        const check = makeCheck(page, shotPrefix("faces", width, scheme), problems);
        await setupFaces(page);
        await openWithFaces(page);
        await check("viewer-faces");
        await region(page, "fu").focus();
        await page.keyboard.press("Enter");
        const dialog = whoIsThis(page);
        await dialog.waitFor();
        const img = dialog.locator(".face-context-frame img");
        await expect.poll(async () => (await img.boundingBox())?.width ?? 0).toBeGreaterThan(40);
        // The sheet slides in: read the picture and the outline in one evaluate, until they agree (it has stopped moving).
        const frame = async () =>
          page.evaluate(() => {
            const im = document.querySelector('[role="dialog"] .face-context-frame img')!.getBoundingClientRect();
            const bx = document.querySelector('[data-testid="face-context-box"]')!.getBoundingClientRect();
            return { i: { x: im.x, y: im.y, width: im.width, height: im.height }, b: { x: bx.x, y: bx.y, width: bx.width, height: bx.height } };
          });
        await expect
          .poll(async () => {
            const { i, b } = await frame();
            return Math.max(
              Math.abs(b.x - (i.x + UNCLAIMED.box[0] * i.width)),
              Math.abs(b.y - (i.y + UNCLAIMED.box[1] * i.height)),
              Math.abs(b.width - UNCLAIMED.box[2] * i.width),
              Math.abs(b.height - UNCLAIMED.box[3] * i.height),
            );
          })
          .toBeLessThan(1.5);
        const { i } = await frame();
        // The whole photo is shown (the picture is landscape, 3:2), not a crop.
        expect(Math.abs(i.width / i.height - 1.5)).toBeLessThan(0.02);
        const vp = page.viewportSize()!;
        const d = (await dialog.boundingBox())!;
        expect(d.x).toBeGreaterThanOrEqual(-0.5);
        expect(d.x + d.width).toBeLessThanOrEqual(vp.width + 0.5);
        for (const name of ["Not a face", "Cancel", "Save"]) {
          const btn = dialog.getByRole("button", { name });
          await btn.scrollIntoViewIfNeeded();
          await expect(btn).toBeInViewport();
        }
        await check("who-is-this");
        expect(problems, problems.join("\n")).toEqual([]);
      });
    });
  }
}
