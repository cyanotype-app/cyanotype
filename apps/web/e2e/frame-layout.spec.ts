// The /frame photo-frame view in a real browser, under the gateway's Content-Security-Policy: on a portrait and a landscape
// tablet (and a phone and a desktop) the photo is fitted over a blurred, darkened copy of itself with the clock and caption
// above it; the tap menu; the setup screen in light and dark; and what happens when the network drops or the session ends.
// Set SHOT_DIR to save screenshots.
import { expect, test, type Page } from "@playwright/test";
import { json, makeCheck, setupApp, shotPrefix, type Handler } from "./kit";
import { TEST_S3_ORIGIN } from "./csp";
import type { FakeAsset } from "./fakeLibrary";
import { installSlidePictures, pictureBox, slideServer, watchViolations } from "./slideshowKit";

const ALBUM_ID = "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d";
const SCREENS = [
  { name: "tablet-portrait", width: 768, height: 1024 },
  { name: "tablet-landscape", width: 1024, height: 768 },
  { name: "tablet-landscape-wide", width: 1280, height: 800 },
  { name: "phone-320", width: 320, height: 568 },
  { name: "phone-414", width: 414, height: 896 },
];

async function setup(page: Page, opts: { extra?: Handler[] } = {}) {
  const violations = await watchViolations(page);
  let assets: FakeAsset[] = [];
  const slide = slideServer(() => assets);
  const handlers: Handler[] = [
    slide.handler,
    (route, url, method) => {
      if (url.pathname === "/assets/albums" && method === "GET") {
        void route.fulfill(json({ items: [{ id: ALBUM_ID, title: "Lake trip", photo_count: 30 }] }));
        return true;
      }
      return false;
    },
    ...(opts.extra ?? []),
  ];
  assets = await setupApp(page, { count: 30, handlers });
  for (const a of assets) a.thumb_url = `${TEST_S3_ORIGIN}${a.thumb_url}`;
  await installSlidePictures(page);
  await page.addInitScript(() => {
    (window as unknown as { __wake: { requested: number } }).__wake = { requested: 0 };
    Object.defineProperty(navigator, "wakeLock", {
      configurable: true,
      value: {
        request: async () => {
          (window as unknown as { __wake: { requested: number } }).__wake.requested++;
          return { released: false, release: async () => undefined };
        },
      },
    });
  });
  return { assets, server: slide.state, violations };
}

const layers = (page: Page) => page.locator("[data-frame] .slide-layer");
const waitPicture = (page: Page) =>
  page.waitForFunction(() => {
    const img = Array.from(document.querySelectorAll<HTMLImageElement>("[data-frame] .slide-photo")).at(-1);
    return !!img && img.complete && img.naturalWidth > 0;
  });
const srcOf = (page: Page) => layers(page).last().locator(".slide-photo").getAttribute("src");
const FRAME = "/frame?source=library&interval=5";

for (const { name, width, height } of SCREENS) {
  test.describe(`frame ${name}`, () => {
    test.use({ viewport: { width, height }, hasTouch: true });

    test("photo contained over a blurred, darkened copy, with the clock and caption", async ({ page }) => {
      test.setTimeout(90_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix(`frame-${name}`, width, "dark"), problems);
      const { server, violations } = await setup(page);
      await page.goto(`${FRAME}&clock=1&caption=1`);
      await waitPicture(page);
      // Random: the feed was asked for a shuffle of the whole library, a seed at a time.
      expect(server.feed[0]).toContain("order=shuffle");
      expect(decodeURIComponent(server.feed[0]!)).toContain('scope="library"');
      expect(server.feed[0]).toMatch(/seed=[0-9a-z]+/);
      await page.waitForTimeout(1000);

      for (let i = 0; i < 4; i++) {
        const b = (await pictureBox(page, "[data-frame]"))!;
        const tol = 1.5;
        expect(b.left).toBeGreaterThanOrEqual(-tol);
        expect(b.top).toBeGreaterThanOrEqual(-tol);
        expect(b.right).toBeLessThanOrEqual(b.vw + tol);
        expect(b.bottom).toBeLessThanOrEqual(b.vh + tol);
        expect(Math.abs(b.w - b.vw) <= tol || Math.abs(b.h - b.vh) <= tol, `fills the screen (${Math.round(b.w)}x${Math.round(b.h)} in ${b.vw}x${b.vh})`).toBe(true);
        // Behind it: the same picture, covering the whole screen, blurred and darkened.
        const bg = await page.evaluate(() => {
          const el = Array.from(document.querySelectorAll<HTMLImageElement>("[data-frame] .slide-backdrop")).at(-1)!;
          const r = el.getBoundingClientRect();
          const cs = getComputedStyle(el);
          return { covers: r.left <= 0 && r.top <= 0 && r.right >= innerWidth && r.bottom >= innerHeight, filter: cs.filter, fit: cs.objectFit, src: el.getAttribute("src") };
        });
        expect(bg.covers).toBe(true);
        expect(bg.filter).toMatch(/blur\(/);
        expect(bg.filter).toMatch(/brightness\(0\.4/);
        expect(bg.fit).toBe("cover");
        expect(bg.src).toBe(b.src);
        await check(`frame-${i}`);
        // The overlays stay on screen, over the lower edge.
        const info = await page.locator("[data-frame-info]").boundingBox();
        expect(info!.x).toBeGreaterThanOrEqual(0);
        expect(info!.x + info!.width).toBeLessThanOrEqual(width + 0.5);
        expect(info!.y + info!.height).toBeLessThanOrEqual(height + 0.5);
        await expect(page.locator("[data-frame-time]")).toBeVisible();
        const before = await srcOf(page);
        await expect.poll(() => srcOf(page), { timeout: 9000 }).not.toBe(before);
        await waitPicture(page);
        await page.waitForTimeout(1000);
      }
      expect(await page.evaluate(() => (window as unknown as { __wake: { requested: number } }).__wake.requested)).toBeGreaterThan(0);
      // Never more than two photos (each with its blurred copy) in the page, however long it runs.
      expect(await page.locator("[data-frame] img").count()).toBeLessThanOrEqual(4);
      expect(await page.locator("[data-frame-menu]").count()).toBe(0);
      expect(violations, "CSP violations").toEqual([]);
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}

test.describe("frame behaviour", () => {
  test.use({ viewport: { width: 1024, height: 768 }, hasTouch: true });

  test("a tap opens a small menu that leads out; it closes by itself", async ({ page }) => {
    test.setTimeout(60_000);
    const problems: string[] = [];
    const check = makeCheck(page, shotPrefix("frame-menu", 1024, "dark"), problems);
    await setup(page);
    await page.goto("/frame?source=library&interval=3600");
    await waitPicture(page);
    await page.touchscreen.tap(500, 300);
    const menu = page.getByRole("dialog", { name: "Photo frame menu" });
    await menu.waitFor();
    await check("menu");
    await expect(menu.getByRole("button", { name: "Change what is shown" })).toBeVisible();
    await expect(menu.getByRole("button", { name: "Exit the frame" })).toBeVisible();
    await expect(menu).toBeHidden({ timeout: 14_000 });
    await page.touchscreen.tap(500, 300);
    await menu.getByRole("button", { name: "Exit the frame" }).click();
    await expect(page).toHaveURL(/\/photos$/);
    expect(problems, problems.join("\n")).toEqual([]);
  });

  test("offline: the last photo stays and says so; back online it carries on", async ({ page, context }) => {
    test.setTimeout(90_000);
    const { server } = await setup(page);
    await page.goto(FRAME);
    await waitPicture(page);
    // Let the preloaded photos run out: the feed holds 30 and previews were fetched three ahead.
    server.down = true;
    await context.setOffline(true);
    await page.waitForTimeout(11_000);
    // Whatever happened to the photos loaded ahead, a photo is on screen all the time.
    for (let i = 0; i < 4; i++) {
      expect(await layers(page).count()).toBeGreaterThan(0);
      expect(await pictureBox(page, "[data-frame]")).not.toBeNull();
      await page.waitForTimeout(2500);
    }
    await expect(page.getByText("Offline. Trying again…").first()).toBeVisible({ timeout: 30_000 });
    expect(await pictureBox(page, "[data-frame]")).not.toBeNull();
    const shown = await srcOf(page);
    expect(shown).toBeTruthy();
    // No sign-in screen, no blank page.
    await expect(page.getByRole("heading", { name: "Sign in again" })).toHaveCount(0);
    server.down = false;
    await context.setOffline(false);
    await expect(page.getByText("Offline. Trying again…")).toBeHidden({ timeout: 20_000 });
    await expect.poll(() => srcOf(page), { timeout: 20_000 }).not.toBe(shown);
  });

  test("a session that ended shows the Sign in again screen, and signing in comes back to the frame", async ({ page }) => {
    test.setTimeout(60_000);
    const { server } = await setup(page);
    await page.goto(FRAME);
    await waitPicture(page);
    // The server now refuses the session everywhere.
    await page.route("**/auth/me", (r) => r.fulfill(json({ error: "Not signed in" }, 401)));
    server.expired = true;
    await expect(page.getByRole("heading", { name: "Sign in again" })).toBeVisible({ timeout: 40_000 });
    const link = page.getByRole("link", { name: "Sign in" });
    await expect(link).toBeVisible();
    await link.click();
    await expect(page).toHaveURL(/\/login$/);
  });

  test("signed out before it starts: a clear screen, not a blank page", async ({ page }) => {
    await setup(page);
    await page.route("**/auth/me", (r) => r.fulfill(json({ error: "Not signed in" }, 401)));
    await page.goto(FRAME);
    await expect(page.getByRole("heading", { name: "Sign in again" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Sign in" })).toBeVisible();
  });

  test("a tablet that wakes before its Wi-Fi waits instead of claiming to be signed out", async ({ page }) => {
    test.setTimeout(60_000);
    await setup(page);
    let up = false;
    await page.route("**/auth/me", (r) =>
      up ? r.fulfill(json({ id: "u1", email: "me@example.org", role: "member" })) : r.abort("connectionfailed"),
    );
    await page.goto(FRAME);
    await expect(page.getByRole("heading", { name: "Waiting for the network" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Sign in again" })).toHaveCount(0);
    up = true;
    await waitPicture(page);
  });

  test("an archived photo is never shown", async ({ page }) => {
    test.setTimeout(60_000);
    const { assets } = await setup(page);
    // Every odd photo is archived (a listing of an album would include them; the frame must not).
    assets.forEach((a, i) => ((a as unknown as { archived: boolean }).archived = i % 2 === 1));
    await page.goto(`/frame?album=${ALBUM_ID}&interval=5`);
    await waitPicture(page);
    const seen = new Set<string>();
    for (let i = 0; i < 5; i++) {
      seen.add((await srcOf(page))!);
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await page.waitForTimeout(5500);
    }
    for (const s of seen) expect(Number(/slide\/(\d+)\.svg/.exec(s)![1]) % 2, s).toBe(0);
  });
});

for (const scheme of ["light", "dark"] as const) {
  for (const width of [320, 414, 768, 1280]) {
    test.describe(`frame setup ${width}px ${scheme}`, () => {
      test.use({ colorScheme: scheme, viewport: { width, height: width < 600 ? 740 : 900 }, hasTouch: width < 600 });

      test("choose what to show, then start", async ({ page }) => {
        test.setTimeout(60_000);
        const problems: string[] = [];
        const check = makeCheck(page, shotPrefix("frame-setup", width, scheme), problems);
        const { server, violations } = await setup(page);
        await page.goto("/frame");
        await page.getByRole("heading", { name: "Photo frame" }).waitFor();
        await check("setup");
        await page.getByLabel("One album").check();
        await page.getByLabel("Album", { exact: true }).selectOption(ALBUM_ID);
        await page.getByLabel("Change the photo every").selectOption("10");
        await page.getByLabel("Show the time and date").check();
        await check("setup-album");
        await page.getByRole("button", { name: "Start the frame" }).click();
        await waitPicture(page);
        await expect(page).toHaveURL(new RegExp(`/frame\\?album=${ALBUM_ID}&interval=10&clock=1`));
        expect(decodeURIComponent(server.feed[0]!)).toContain(`scope={"album":"${ALBUM_ID}"}`);
        // A reload (the tablet restarted) starts it again from the address alone.
        await page.reload();
        await waitPicture(page);
        await check("frame-after-setup");
        expect(violations, "CSP violations").toEqual([]);
        expect(problems, problems.join("\n")).toEqual([]);
      });
    });
  }
}
