// The virtualized timeline in a real browser, against a mocked library of 20,000 photos over seven years: how many
// tiles exist in the DOM, how the year/month scrubber jumps, favorites, and downloads. Set SHOT_DIR to also save
// screenshots (390 px phone and 1280 px desktop).
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { installLibrary, makeAssets, startBucket, type FakeAsset } from "./fakeLibrary";

const SHOT_DIR = process.env.SHOT_DIR;
const TOTAL = 20_000;
const MAX_TILES = 300; // "a few hundred" at most, in any scroll position

let assets: FakeAsset[];
test.beforeEach(() => {
  assets = makeAssets(TOTAL);
});

async function setup(page: Page, bucketOrigin?: string) {
  await page.route("**/auth/me", (r) =>
    r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ id: "u1", email: "me@example.org" }) }),
  );
  return installLibrary(page, assets, (r) => r.fallback(), bucketOrigin);
}

const shot = async (page: Page, name: string) => {
  if (!SHOT_DIR) return;
  mkdirSync(SHOT_DIR, { recursive: true });
  await page.screenshot({ path: join(SHOT_DIR, `${page.viewportSize()!.width}-${name}.png`) });
};
const tiles = (page: Page) => page.locator(".tile-cell").count();
const loadedTiles = (page: Page) => page.locator(".tile-cell img").count();
const heapMb = (page: Page) =>
  page.evaluate(() => Math.round(((performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? 0) / 1e6));

/** Drag the scrubber handle to `fraction` of the rail without letting go (touch or mouse). */
async function dragHandle(page: Page, fraction: number, { touch = false } = {}) {
  const handle = page.getByRole("slider");
  const rail = (await page.locator(".scrubber").boundingBox())!;
  const h = (await handle.boundingBox())!;
  const x = h.x + h.width / 2;
  const startY = h.y + h.height / 2;
  const endY = rail.y + 15 + fraction * (rail.height - 30);
  if (touch) {
    const cdp = await page.context().newCDPSession(page);
    const pt = (y: number) => [{ x, y, id: 1 }];
    await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: pt(startY) });
    for (let i = 1; i <= 8; i++) {
      await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: pt(startY + ((endY - startY) * i) / 8) });
    }
    return async () => {
      await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      await cdp.detach();
    };
  }
  await page.mouse.move(x, startY);
  await page.mouse.down();
  await page.mouse.move(x, (startY + endY) / 2, { steps: 4 });
  await page.mouse.move(x, endY, { steps: 4 });
  return async () => {
    await page.mouse.up();
  };
}

for (const { width, height, touch } of [
  { width: 1280, height: 800, touch: false },
  { width: 390, height: 780, touch: true },
]) {
  test.describe(`${width}px`, () => {
    test.use({ viewport: { width, height }, hasTouch: touch, isMobile: touch, deviceScaleFactor: touch ? 2 : 1 });

    test("20,000 photos: only a few hundred tiles ever exist, at the top and after scrubber jumps", async ({ page }) => {
      test.setTimeout(120_000);
      const seen = await setup(page);
      await page.goto("/library");
      await expect(page.locator(".tile-cell img").first()).toBeVisible();
      await page.waitForLoadState("networkidle");

      // Initial render: the whole library is laid out, but only the rows near the viewport are in the DOM.
      const first = await tiles(page);
      console.log(`[${width}px] initial: ${first} tile cells (${await loadedTiles(page)} with images), ${await page.locator(".tl-row").count()} rows`);
      expect(first).toBeGreaterThan(5);
      expect(first).toBeLessThanOrEqual(MAX_TILES);
      expect(await page.locator("h2").first().textContent()).toBe("October 2026");
      expect(await page.locator("h3").first().textContent()).toBe("Wed, Oct 14");
      await expect(page.getByText("20,000 photos")).toBeAttached();
      // The document is as tall as 20,000 photos need, not as tall as what was rendered.
      const docHeight = await page.evaluate(() => document.documentElement.scrollHeight);
      expect(docHeight).toBeGreaterThan(20_000 / 6 * 150);
      expect(seen.timeline).toHaveLength(1);
      expect(seen.list.length).toBeLessThanOrEqual(2);
      const before = await heapMb(page);

      await shot(page, "timeline");
      await expect(page.locator(".scrubber")).toBeVisible();
      const years = await page.locator(".scrub-year").allTextContents();
      expect(years[0]).toBe("2026");
      expect(years.length).toBeGreaterThanOrEqual(6);

      // Drag the handle half way: a bubble names the month, the page jumps there, and only that part is fetched.
      const lists = seen.list.length;
      const release = await dragHandle(page, 0.5, { touch });
      const bubble = page.locator(".scrubber-bubble");
      await expect(bubble).toBeVisible();
      const label = (await bubble.textContent())!;
      expect(label).toMatch(/^[A-Z][a-z]+ 20(19|2\d)$/);
      console.log(`[${width}px] dragging to 50%: "${label}"`);
      await shot(page, "scrubber-dragging");
      await release();
      await expect(bubble).toBeHidden();
      await expect(page.locator(".tile-cell img").first()).toBeVisible({ timeout: 10_000 });
      await page.waitForLoadState("networkidle");
      const mid = await tiles(page);
      console.log(`[${width}px] after jump to "${label}": ${mid} tile cells, ${seen.list.length - lists} list requests`);
      expect(mid).toBeLessThanOrEqual(MAX_TILES);
      // Nothing before the target was loaded to get there: a couple of requests, not hundreds.
      expect(seen.list.length - lists).toBeLessThanOrEqual(3);
      expect(seen.list.at(-1)).toMatch(/before=|cursor=/);
      expect(await page.evaluate(() => window.scrollY)).toBeGreaterThan(docHeight * 0.3);
      await expect(page.getByRole("slider")).toHaveAttribute("aria-valuetext", /20(19|2\d)/);
      await shot(page, "timeline-after-jump");

      // Tap a year on the rail: lands on the top of its first month.
      const y2022 = page.locator(".scrub-year", { hasText: "2022" });
      await y2022.click();
      await expect(page.locator(".tile-cell img").first()).toBeVisible({ timeout: 10_000 });
      await page.waitForLoadState("networkidle");
      const heading = page.locator("h2", { hasText: "2022" }).first();
      await expect(heading).toBeVisible();
      const top = await heading.evaluate((e) => e.getBoundingClientRect().top);
      expect(top).toBeLessThan(200);
      expect(await tiles(page)).toBeLessThanOrEqual(MAX_TILES);

      // Jump to the very end and back to the start.
      const end = await dragHandle(page, 1, { touch });
      await end();
      await expect(page.locator(".tile-cell img").first()).toBeVisible({ timeout: 10_000 });
      await page.waitForLoadState("networkidle");
      expect(await tiles(page)).toBeLessThanOrEqual(MAX_TILES);
      await expect(page.getByRole("slider")).toHaveAttribute("aria-valuetext", /20(19|20)/);
      // The oldest photos are actually there (the last page, loaded without anything before it).
      await expect(page.getByAltText(`IMG_${String(TOTAL - 1).padStart(5, "0")}.jpg`)).toBeVisible();
      await shot(page, "timeline-end");
      const start = await dragHandle(page, 0, { touch });
      await start();
      await expect(page.locator("h2").first()).toHaveText("October 2026");

      // Scrolling the page with the wheel / a swipe through a stretch of the library keeps the DOM small too.
      let worst = 0;
      for (let i = 0; i < 12; i++) {
        await page.evaluate(() => window.scrollBy(0, 1800));
        await page.waitForTimeout(60);
        worst = Math.max(worst, await tiles(page));
      }
      await page.waitForLoadState("networkidle");
      worst = Math.max(worst, await tiles(page));
      console.log(`[${width}px] scrolling: at most ${worst} tile cells; heap ${before} MB -> ${await heapMb(page)} MB; ${seen.list.length} list requests in total`);
      expect(worst).toBeLessThanOrEqual(MAX_TILES);
      expect(await heapMb(page)).toBeLessThan(250);
    });

    test("favorites: heart on tiles and in the lightbox, bulk favorite, and the Favorites view", async ({ page }) => {
      const small = makeAssets(400);
      assets.splice(0, assets.length, ...small);
      const seen = await setup(page);
      await page.goto("/library");
      await expect(page.getByRole("button", { name: /Open IMG_00000/ })).toBeVisible();

      // A favorited photo (i % 11 === 3) shows its heart without hovering; others only on hover.
      const fav = page.getByRole("button", { name: "Remove IMG_00003.jpg from favorites" });
      await expect(fav).toBeVisible();
      const plain = page.getByRole("button", { name: "Add IMG_00001.jpg to favorites" });
      if (!touch) {
        await expect(plain).toHaveCSS("opacity", "0");
        await page.getByRole("button", { name: "Open IMG_00001.jpg" }).hover();
        await expect(plain).toHaveCSS("opacity", "1");
        await shot(page, "favorites-heart-hover");
        await plain.click();
        expect(seen.favorites.at(-1)).toEqual({ ids: ["a1"], favorite: true });
        await expect(page.getByRole("button", { name: "Remove IMG_00001.jpg from favorites" })).toBeVisible();
      }
      await shot(page, "favorites-tiles");

      // Bulk: select two, Favorite.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: "Select IMG_00005.jpg" }).click();
      await page.getByRole("button", { name: "Select IMG_00006.jpg" }).click();
      await shot(page, "favorites-selection-bar");
      await page.getByRole("button", { name: "Favorite", exact: true }).click();
      expect(seen.favorites.at(-1)).toEqual({ ids: ["a5", "a6"], favorite: true });
      await expect(page.getByText("2 added to favorites")).toBeVisible();

      // Lightbox heart.
      await page.getByRole("button", { name: "Open IMG_00002.jpg" }).click();
      const box = page.getByRole("dialog");
      await box.getByRole("button", { name: "Add to favorites" }).click();
      await expect(box.getByRole("button", { name: "Remove from favorites" })).toHaveAttribute("aria-pressed", "true");
      expect(seen.favorites.at(-1)).toEqual({ ids: ["a2"], favorite: true });
      await shot(page, "lightbox-favorited");
      await page.keyboard.press("Escape");

      // The Favorites view only has hearts, and the nav links to it.
      await page.getByRole("link", { name: "Favorites" }).click();
      await expect(page).toHaveURL(/\/favorites$/);
      await expect(page.getByRole("link", { name: "Favorites" })).toHaveAttribute("aria-current", "page");
      const hearted = assets.filter((a) => a.favorite).length;
      await expect(page.getByText(`${hearted} photos`)).toBeAttached();
      expect(seen.timeline.at(-1)).toBe("?favorite=true");
      await expect(page.locator(".tile-cell img").first()).toBeVisible();
      await page.waitForLoadState("networkidle");
      expect(await page.getByRole("button", { name: /^Remove .* from favorites$/ }).count()).toBe(await tiles(page));
      await shot(page, "favorites-view");
    });

    test("download: the original is saved straight from the bucket by a plain navigation", async ({ page }) => {
      const small = makeAssets(60);
      assets.splice(0, assets.length, ...small);
      const bucket = await startBucket();
      const seen = await setup(page, bucket.origin);
      await page.goto("/library");
      // a0 is a RAW + JPEG pair; a1 is a single file.
      await page.getByRole("button", { name: "Open IMG_00001.jpg" }).click();
      const box = page.getByRole("dialog");
      await expect(box.getByRole("link", { name: "Download" })).toHaveAttribute("href", "/assets/a1/download");
      await shot(page, "lightbox-download");
      const [single] = await Promise.all([page.waitForEvent("download"), box.getByRole("link", { name: "Download" }).click()]);
      expect(single.suggestedFilename()).toBe("IMG_00001.jpg");
      expect(page.url()).toMatch(/\/library$/); // the page stayed where it was
      await page.keyboard.press("Escape");

      await page.getByRole("button", { name: "Open IMG_00000.jpg" }).click();
      await box.getByRole("button", { name: "Download" }).click();
      await expect(box.getByRole("link", { name: /Download RAW/ })).toHaveAttribute("href", "/assets/a0/download?file=r0");
      await shot(page, "lightbox-download-pair");
      const [raw] = await Promise.all([page.waitForEvent("download"), box.getByRole("link", { name: /Download RAW/ }).click()]);
      expect(raw.suggestedFilename()).toBe("IMG_00000.CR3");
      const [jpg] = await Promise.all([page.waitForEvent("download"), box.getByRole("link", { name: /Download JPEG/ }).click()]);
      expect(jpg.suggestedFilename()).toBe("IMG_00000.jpg");

      // No CORS needed: the bucket sent no CORS headers and was only ever reached by navigations, never fetch()/XHR.
      expect(seen.favorites).toEqual([]);
      expect(bucket.requests).toHaveLength(3);
      for (const r of bucket.requests) {
        expect(r.mode).toBe("navigate");
        expect(r.dest).toBe("document");
        expect(decodeURIComponent(r.url)).toContain("response-content-disposition=attachment; filename=");
      }
      await bucket.close();
    });
  });
}
