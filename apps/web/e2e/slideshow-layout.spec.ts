// The slideshow in a real browser, under the gateway's Content-Security-Policy: started from an album, a search and the viewer,
// at phone and desktop widths in light and dark. The picture fits the screen whatever its shape, the controls fit and hide
// themselves, the keys work, a crossfade leaves one layer behind, reduced motion cuts, and where the browser has no full screen
// (an iPhone) the slideshow fills the window instead. Set SHOT_DIR to save screenshots.
import { expect, test, type Page } from "@playwright/test";
import { json, makeCheck, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";
import { TEST_S3_ORIGIN } from "./csp";
import type { FakeAsset } from "./fakeLibrary";
import { installSlidePictures, pictureBox, slideServer, watchViolations } from "./slideshowKit";

const RUNS = [320, 375, 414, 1280].flatMap((width) => (["light", "dark"] as const).map((scheme) => ({ width, scheme })));
const ALBUM = {
  id: "alb1",
  title: "Summer by the lake with a really quite long album name that goes on and on and on",
  description: null,
  smart: false,
  rules: null,
  photo_count: 30,
  hidden_count: 0,
  cover: { id: "a1", thumbhash: null, thumb_url: "/fixture-thumbs/1.svg" },
  first_at: "2026-01-01T00:00:00Z",
  last_at: "2026-10-01T00:00:00Z",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

async function setup(page: Page, opts: { duration?: number; extra?: Handler[] } = {}) {
  const violations = await watchViolations(page);
  let assets: FakeAsset[] = [];
  const slide = slideServer(() => assets);
  const handlers: Handler[] = [
    slide.handler,
    (route, url, method) => {
      const p = url.pathname;
      const ok = (body: unknown) => {
        void route.fulfill(json(body));
        return true;
      };
      if (p === "/assets/albums/alb1" && method === "GET") return ok(ALBUM);
      if (p === "/assets/albums/alb1/assets") return ok({ items: assets, next_cursor: null, next_before: null });
      if (p === "/assets/search/facets")
        return ok({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [] });
      if (p === "/assets/search" && method === "GET")
        return ok({ items: assets.slice(0, 12), next_cursor: null, ranked: !!url.searchParams.get("q"), total: 12 });
      return false;
    },
    ...(opts.extra ?? []),
  ];
  assets = await setupApp(page, { count: 30, handlers });
  for (const a of assets) a.thumb_url = `${TEST_S3_ORIGIN}${a.thumb_url}`;
  await installSlidePictures(page);
  // The wake lock is recorded, not granted (headless Chromium has none).
  await page.addInitScript((duration) => {
    const w = window as unknown as { __wake: { requested: number; released: number } };
    w.__wake = { requested: 0, released: 0 };
    Object.defineProperty(navigator, "wakeLock", {
      configurable: true,
      value: {
        request: async () => {
          w.__wake.requested++;
          return { released: false, release: async () => void w.__wake.released++ };
        },
      },
    });
    if (duration) localStorage.setItem("cyanotype.slideshow", JSON.stringify({ duration, order: "listed", transition: "crossfade", loop: true }));
  }, opts.duration ?? 0);
  return { assets, server: slide.state, violations };
}

const current = (page: Page) => page.locator(".slideshow .slide-layer").last().locator(".slide-photo");
const waitPicture = async (page: Page) => {
  await page.waitForFunction(() => {
    const img = Array.from(document.querySelectorAll<HTMLImageElement>(".slideshow .slide-photo")).at(-1);
    return !!img && img.complete && img.naturalWidth > 0;
  });
};
const srcOf = (page: Page) => current(page).getAttribute("src");

/** The picture lies inside the screen and fills its width or height (it is contained, not cropped or stretched). */
async function expectFits(page: Page, label: string) {
  const b = (await pictureBox(page, ".slideshow"))!;
  expect(b, `${label}: a picture`).not.toBeNull();
  const tol = 1.5;
  expect(b.left, `${label} left`).toBeGreaterThanOrEqual(-tol);
  expect(b.top, `${label} top`).toBeGreaterThanOrEqual(-tol);
  expect(b.right, `${label} right`).toBeLessThanOrEqual(b.vw + tol);
  expect(b.bottom, `${label} bottom`).toBeLessThanOrEqual(b.vh + tol);
  expect(Math.abs(b.w / b.h - b.nw / b.nh), `${label} keeps its shape`).toBeLessThan(0.01);
  expect(Math.abs(b.w - b.vw) <= tol || Math.abs(b.h - b.vh) <= tol, `${label} fills the screen (${Math.round(b.w)}x${Math.round(b.h)} in ${b.vw}x${b.vh})`).toBe(true);
}

/** Every visible control lies inside the screen. */
async function expectControlsOnScreen(page: Page, label: string) {
  const out = await page.evaluate(() =>
    Array.from(document.querySelectorAll<HTMLElement>(".slideshow button, .slideshow select"))
      .filter((e) => e.getBoundingClientRect().width > 0)
      .filter((e) => {
        const b = e.getBoundingClientRect();
        return b.left < -0.5 || b.right > window.innerWidth + 0.5 || b.top < -0.5 || b.bottom > window.innerHeight + 0.5;
      })
      .map((e) => e.getAttribute("aria-label") || e.textContent),
  );
  expect(out, `${label}: controls outside the screen`).toEqual([]);
}

for (const { width, scheme } of RUNS) {
  test.describe(`slideshow ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("from an album: fits, keys, settings, no overflow", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("slideshow", width, scheme), problems);
      const { server, violations } = await setup(page);

      await page.goto("/albums/alb1");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await check("album-with-slideshow-button");
      await page.getByRole("button", { name: "Slideshow", exact: true }).click();
      const show = page.getByRole("dialog", { name: /^Slideshow: / });
      await show.waitFor();
      await waitPicture(page);
      await check("slideshow");
      await expectFits(page, "first photo");
      await expectControlsOnScreen(page, "controls");
      // The server's ordered feed for exactly this album, as listed; previews, never originals.
      expect(server.feed[0]).toContain("order=listed");
      expect(decodeURIComponent(server.feed[0]!)).toContain('scope={"album":"alb1"}');
      expect(server.previews.length).toBeGreaterThan(0);

      // Each shape fits (landscape, portrait, panorama, square).
      for (let i = 1; i <= 4; i++) {
        const before = await srcOf(page);
        await page.keyboard.press("ArrowRight");
        await expect.poll(() => srcOf(page)).not.toBe(before);
        await waitPicture(page);
        await page.waitForTimeout(1000); // the crossfade ends
        await expectFits(page, `photo ${i + 1}`);
      }
      await check("slideshow-later-photo");

      // Space pauses, Space plays; the arrow keys go back.
      await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
      await page.keyboard.press("Space");
      await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
      await page.keyboard.press("Space");
      await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
      const here = await srcOf(page);
      await page.keyboard.press("ArrowLeft");
      await expect.poll(() => srcOf(page)).not.toBe(here);

      // Settings: the panel fits, and a shuffle is asked of the server.
      await page.getByRole("button", { name: "Slideshow settings" }).click();
      const panel = page.getByRole("group", { name: "Slideshow settings" });
      await panel.waitFor();
      await check("slideshow-settings");
      await expectControlsOnScreen(page, "settings");
      await panel.getByLabel("Order").selectOption("shuffle");
      await expect.poll(() => server.feed.some((q) => q.includes("order=shuffle") && q.includes("seed="))).toBe(true);
      await panel.getByLabel("Time per photo").selectOption("20");
      expect(await page.evaluate(() => JSON.parse(localStorage.getItem("cyanotype.slideshow")!))).toMatchObject({ duration: 20, order: "shuffle" });

      // Escape closes the panel first, then the slideshow, and the page is as it was.
      await page.keyboard.press("Escape");
      await expect(panel).toBeHidden();
      await page.keyboard.press("Escape");
      await expect(show).toBeHidden();
      await expect(page.getByRole("heading", { name: /Summer by the lake/ })).toBeVisible();
      await check("album-after");
      expect(violations, "CSP violations").toEqual([]);
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("from search results (filter-only and ranked)", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("slideshow-search", width, scheme), problems);
      const { server, violations } = await setup(page);

      // Only filters: a scope the server orders and pages.
      await page.goto("/search?from=2026-01-01&favorite=1");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByRole("button", { name: "Slideshow", exact: true }).click();
      await page.getByRole("dialog", { name: /^Slideshow: / }).waitFor();
      await waitPicture(page);
      await expectFits(page, "filter search");
      expect(decodeURIComponent(server.feed[0]!)).toContain('"search":{');
      expect(decodeURIComponent(server.feed[0]!)).toContain('"from":"2026-01-01"');
      await page.keyboard.press("Escape");

      // Words: read through the search's own pages.
      server.feed.length = 0;
      await page.goto("/search?q=lake");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await check("search-results");
      await page.getByRole("button", { name: "Slideshow", exact: true }).click();
      await page.getByRole("dialog", { name: /^Slideshow: / }).waitFor();
      await waitPicture(page);
      await check("slideshow-search");
      await expectFits(page, "ranked search");
      expect(server.feed).toEqual([]);
      await page.keyboard.press("Escape");
      expect(violations, "CSP violations").toEqual([]);
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("the controls hide when nothing moves and come back on a touch or the mouse", async ({ page }) => {
      test.setTimeout(60_000);
      const { violations } = await setup(page);
      await page.goto("/albums/alb1");
      await page.getByRole("button", { name: "Slideshow", exact: true }).click();
      const show = page.getByRole("dialog", { name: /^Slideshow: / });
      await waitPicture(page);
      const bar = show.locator(".slideshow-bottom");
      await expect(bar).toHaveCSS("opacity", "1");
      await expect(show).toHaveClass(/idle/, { timeout: 8000 });
      await expect(bar).toHaveCSS("opacity", "0");
      await expect(bar).toHaveCSS("pointer-events", "none");
      if (width < 600) await page.touchscreen.tap(width / 2, 300);
      else await page.mouse.move(width / 2, 300);
      await expect(show).not.toHaveClass(/idle/);
      await expect(bar).toHaveCSS("opacity", "1");
      // Paused, they stay.
      await page.keyboard.press("Space");
      await page.waitForTimeout(4500);
      await expect(show).not.toHaveClass(/idle/);
      expect(violations, "CSP violations").toEqual([]);
    });
  });
}

test.describe("slideshow behaviour", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("plays by itself, crossfades, and keeps the screen awake while it runs", async ({ page }) => {
    test.setTimeout(60_000);
    const { violations } = await setup(page, { duration: 3 });
    await page.goto("/albums/alb1");
    await page.getByRole("button", { name: "Slideshow", exact: true }).click();
    await waitPicture(page);
    const first = await srcOf(page);
    await expect.poll(() => page.evaluate(() => (window as unknown as { __wake: { requested: number } }).__wake.requested)).toBeGreaterThan(0);
    // After ~3 s the next photo fades in over the first: two layers for a moment, then one.
    await expect.poll(() => page.locator(".slideshow .slide-layer").count(), { timeout: 8000 }).toBe(2);
    expect(await srcOf(page)).not.toBe(first);
    await expect.poll(() => page.locator(".slideshow .slide-layer").count(), { timeout: 4000 }).toBe(1);
    // It never holds more than two layers, however long it runs.
    for (let i = 0; i < 5; i++) {
      await page.waitForTimeout(1500);
      expect(await page.locator(".slideshow img").count()).toBeLessThanOrEqual(2);
    }
    await page.keyboard.press("Escape");
    await expect.poll(() => page.evaluate(() => (window as unknown as { __wake: { released: number } }).__wake.released)).toBeGreaterThan(0);
    expect(violations, "CSP violations").toEqual([]);
  });

  test("reduced motion: the next photo replaces the last with no layer left over", async ({ page }) => {
    test.setTimeout(60_000);
    await page.emulateMedia({ reducedMotion: "reduce" });
    await setup(page, { duration: 3 });
    await page.goto("/albums/alb1");
    await page.getByRole("button", { name: "Slideshow", exact: true }).click();
    await waitPicture(page);
    const first = await srcOf(page);
    await expect.poll(() => srcOf(page), { timeout: 8000 }).not.toBe(first);
    expect(await page.locator(".slideshow .slide-layer").count()).toBe(1);
    await page.mouse.move(600, 400); // the controls have hidden themselves by now
    await page.getByRole("button", { name: "Slideshow settings" }).click();
    await expect(page.getByLabel("Transition")).toBeDisabled();
  });

  test("with no full screen in the browser (an iPhone) it fills the window and offers no full screen button", async ({ page }) => {
    await page.addInitScript(() => {
      delete (Element.prototype as { requestFullscreen?: unknown }).requestFullscreen;
      Object.defineProperty(document, "fullscreenEnabled", { configurable: true, value: false });
    });
    await setup(page);
    await page.goto("/albums/alb1");
    await page.getByRole("button", { name: "Slideshow", exact: true }).click();
    const show = page.getByRole("dialog", { name: /^Slideshow: / });
    await waitPicture(page);
    await expect(page.getByRole("button", { name: /full screen/i })).toHaveCount(0);
    const box = await show.boundingBox();
    expect(box).toMatchObject({ x: 0, y: 0, width: 1280, height: 800 });
  });

  test("where the browser has full screen, the button toggles it and Escape still closes", async ({ page }) => {
    await setup(page);
    await page.goto("/albums/alb1");
    await page.getByRole("button", { name: "Slideshow", exact: true }).click();
    await waitPicture(page);
    const button = page.getByRole("button", { name: /full screen/i });
    await expect(button).toBeVisible();
    // Started from a click: it went full screen at once.
    await expect.poll(() => page.evaluate(() => !!document.fullscreenElement)).toBe(true);
    await expect(button).toHaveAttribute("aria-pressed", "true");
    await button.click();
    await expect.poll(() => page.evaluate(() => !!document.fullscreenElement)).toBe(false);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: /^Slideshow: / })).toBeHidden();
  });

  test("Play from here in the viewer (S) starts at that photo and Escape comes back to it", async ({ page }) => {
    test.setTimeout(60_000);
    const { violations } = await setup(page);
    await page.goto("/library");
    await page.getByRole("button", { name: /Open IMG_00002/ }).first().click();
    await page.getByRole("dialog", { name: "IMG_00002.jpg" }).waitFor();
    await page.keyboard.press("s");
    await page.getByRole("dialog", { name: /^Slideshow: / }).waitFor();
    await waitPicture(page);
    expect(await srcOf(page)).toContain("/fixture-slide/2.svg");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: /^Slideshow: / })).toBeHidden();
    await expect(page.getByRole("dialog", { name: "IMG_00002.jpg" })).toBeVisible();
    // The button does the same.
    await page.getByRole("button", { name: "Play from here" }).click();
    await page.getByRole("dialog", { name: /^Slideshow: / }).waitFor();
    expect(violations, "CSP violations").toEqual([]);
  });

  test("keeps the picture on screen when the server cannot be reached, and says so", async ({ page }) => {
    test.setTimeout(60_000);
    const { server } = await setup(page, { duration: 3 });
    await page.goto("/albums/alb1");
    await page.getByRole("button", { name: "Slideshow", exact: true }).click();
    await waitPicture(page);
    server.down = true;
    // Run past the photos loaded ahead (the first chunk is all 30): jump near its end.
    for (let i = 0; i < 29; i++) await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowRight");
    await expect(page.getByText(/Can’t reach the server/)).toBeVisible({ timeout: 20_000 });
    const stuck = await srcOf(page);
    expect(stuck).toBeTruthy();
    expect(await pictureBox(page, ".slideshow")).not.toBeNull();
    server.down = false;
    // The browser notices the network is back (the retry waits would otherwise grow to half a minute).
    await page.evaluate(() => window.dispatchEvent(new Event("online")));
    await expect(page.getByText(/Can’t reach the server/)).toBeHidden({ timeout: 20_000 });
  });

  test("a session that ended says so", async ({ page }) => {
    test.setTimeout(60_000);
    const { server } = await setup(page);
    await page.goto("/albums/alb1");
    await page.getByRole("button", { name: "Slideshow", exact: true }).click();
    await waitPicture(page);
    server.expired = true;
    for (let i = 0; i < 30; i++) await page.keyboard.press("ArrowRight");
    await expect(page.getByRole("heading", { name: "Sign in again" })).toBeVisible({ timeout: 20_000 });
  });
});
