// The app under the gateway's real Content-Security-Policy (infra/caddy/csp.caddy, served by `vite preview` through
// playwright.config.ts): walk the library, the viewer, search, the map, albums, a share page, settings and an upload, and
// fail on any `securitypolicyviolation` event or CSP console error. Photos come from a separate origin (TEST_S3_ORIGIN) the
// way presigned S3 URLs do. A last probe proves the policy really blocks a foreign origin, so a green run cannot mean "no
// policy was applied". (The file name puts it in the "layout" project, whose Chromium has software WebGL for the map.)
import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { rgbaToThumbHash } from "thumbhash";
import { json, setupApp, type Handler } from "./kit";
import { spaPolicy, TEST_S3_ORIGIN } from "./csp";

const WORLD_TILES = readFileSync(new URL("./fixtures/world-z2.pmtiles", import.meta.url));
const THUMBHASH = Buffer.from(rgbaToThumbHash(4, 4, new Uint8Array(64).map((_, i) => (i % 4 === 3 ? 255 : (i * 37) % 256)))).toString("base64");
const FOREIGN = "http://evil.csp.test";

const marker = (id: string, lat: number, lon: number, count: number, n: number) => ({
  id,
  lat,
  lon,
  count,
  bounds: count > 1 ? [lon - 0.4, lat - 0.3, lon + 0.4, lat + 0.3] : null,
  thumbhash: null,
  thumb_url: count === 1 ? `${TEST_S3_ORIGIN}/fixture-thumbs/${n}.svg` : null,
});

const pubItem = (n: number) => ({
  id: `p${n}`,
  width: 400,
  height: 300,
  sort_time: new Date(Date.UTC(2026, 0, 30 - n)).toISOString(),
  // A real ThumbHash so the data: URL placeholder path runs.
  thumbhash: THUMBHASH,
  thumb_url: `${TEST_S3_ORIGIN}/fixture-thumbs/${n}.svg`,
  preview_url: `${TEST_S3_ORIGIN}/fixture-thumbs/${n}.svg`,
  downloads: [{ id: `p${n}`, label: "JPEG", size_bytes: null }],
  download_pending: false,
});

const handlers: Handler[] = [
  (route, url, method) => {
    const p = url.pathname;
    const ok = (body: unknown, status = 200) => {
      void route.fulfill(json(body, status));
      return true;
    };
    if (p === "/assets/tiles" && method === "GET") return ok({ world: true, detail: false });
    if (p === "/assets/tiles/world.pmtiles") {
      const r = /bytes=(\d+)-(\d+)/.exec(route.request().headers()["range"] ?? "");
      if (!r) return ok({}, 400);
      const first = Number(r[1]);
      const last = Math.min(Number(r[2]), WORLD_TILES.length - 1);
      void route.fulfill({
        status: 206,
        contentType: "application/octet-stream",
        headers: { "content-range": `bytes ${first}-${last}/${WORLD_TILES.length}`, "accept-ranges": "bytes", etag: '"fixture"' },
        body: WORLD_TILES.subarray(first, last + 1),
      });
      return true;
    }
    if (p === "/assets/map" && method === "GET")
      return ok({ clusters: [marker("m1", 30.04, 31.24, 1, 1), marker("m2", 51.5, -0.12, 7, 2), marker("m3", 40.7, -74.0, 128, 3)], total: 136, truncated: false });
    if (p === "/assets/search/facets") return ok({ total: 30, favorites: 3, with_location: 3, types: [], cameras: [], places: [], years: [] });
    if (p === "/assets/search" && method === "GET")
      return ok({ items: [], next_cursor: null, ranked: true, total: 0, strong_count: 0, weaker: null, parsed: { text: "beach", chips: [] } });
    if (p === "/assets/places") return ok([]);
    if (p === "/assets/check") return ok({ status: "exists", asset_id: "a1" });
    if (p === "/assets/shares") return ok({ items: [] });
    if (p === "/assets/trash" && method === "GET")
      return ok({
        items: [1, 2, 3].map((n) => ({
          id: `t${n}`,
          original_name: `trashed-${n}.jpg`,
          content_type: "image/jpeg",
          size_bytes: 1000,
          width: 400,
          height: 300,
          sort_time: new Date(Date.UTC(2026, 0, 30 - n)).toISOString(),
          thumbhash: null,
          thumb_url: `${TEST_S3_ORIGIN}/fixture-thumbs/${n}.svg`,
          deleted_at: new Date(Date.now() - 86_400_000).toISOString(),
          purge_at: new Date(Date.now() + 29 * 86_400_000).toISOString(),
        })),
        next_cursor: null,
        total: 3,
        retention_days: 30,
      });
    if (p.startsWith("/assets/public/s/")) {
      const rest = p.slice("/assets/public/s/".length).split("/").slice(1).join("/");
      if (rest === "")
        return ok({ needs_password: false, unlocked: true, kind: "album", title: "Summer trip", item_count: 3, allow_download: true, include_originals: false, expires_at: null });
      if (rest === "items") return ok({ items: [1, 2, 3].map(pubItem), next_cursor: null });
    }
    return false;
  },
];

/** Collect every CSP violation the page reports, from the page itself and from its console. */
async function watchViolations(page: Page) {
  const seen: string[] = [];
  await page.exposeFunction("__reportViolation", (s: string) => seen.push(s));
  await page.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (e) => {
      void (window as unknown as { __reportViolation: (s: string) => void }).__reportViolation(
        `${e.violatedDirective} blocked ${e.blockedURI || "inline"} at ${e.sourceFile || "page"}:${e.lineNumber} sample=${e.sample}`,
      );
    });
  });
  page.on("console", (m) => {
    if (/Content Security Policy|Content-Security-Policy/i.test(m.text())) seen.push(`console: ${m.text()}`);
  });
  page.on("pageerror", (e) => {
    if (/Content Security Policy/i.test(e.message)) seen.push(`pageerror: ${e.message}`);
  });
  return seen;
}

test.use({ viewport: { width: 1280, height: 800 } });

test("every page works under the gateway's Content-Security-Policy", async ({ page }) => {
  test.setTimeout(180_000);
  const violations = await watchViolations(page);
  const assets = await setupApp(page, { count: 30, handlers });
  // Photos live on the "bucket" origin, like presigned S3 URLs.
  for (const a of assets) a.thumb_url = `${TEST_S3_ORIGIN}${a.thumb_url}`;

  // The app really is served with the policy (a stale server without it would make this test meaningless).
  const first = await page.goto("/library");
  expect(first?.headers()["content-security-policy"]).toBe(spaPolicy());
  expect(spaPolicy()).toContain(TEST_S3_ORIGIN);
  expect(spaPolicy()).not.toMatch(/'unsafe-(inline|eval)'/);

  // Library and viewer
  const open = page.getByRole("button", { name: /Open IMG_00002/ }).first();
  await open.waitFor();
  await page.waitForLoadState("networkidle");
  await open.click();
  await page.getByRole("dialog", { name: "IMG_00002.jpg" }).waitFor();
  await page.waitForTimeout(300);
  await page.keyboard.press("Escape");

  // Search
  await page.goto("/search?q=beach");
  await page.waitForLoadState("networkidle");

  // Albums
  await page.goto("/albums");
  await page.waitForLoadState("networkidle");

  // Map: MapLibre, its worker, PMTiles range reads and glyphs
  await page.goto("/map");
  await page.getByText("Photos in view").first().waitFor();
  await page.locator(".map-thumb, .map-bubble, .map-notice[role=alert]").first().waitFor({ timeout: 20_000 });
  await page.waitForTimeout(1500);
  expect(await page.locator(".map-canvas canvas, canvas.maplibregl-canvas").count()).toBeGreaterThan(0);

  // Share page (public, no sign-in): ThumbHash placeholders from data: URLs, photos from the bucket origin
  await page.goto("/s/csp-test-token");
  await page.getByText("Summer trip").first().waitFor();
  await page.waitForLoadState("networkidle");

  // Settings pages
  for (const path of ["/settings", "/settings/display", "/settings/storage", "/settings/sessions", "/settings/security", "/settings/api-keys", "/settings/google-photos"]) {
    await page.goto(path);
    await page.waitForLoadState("networkidle");
  }

  // A confirmation sheet (a <dialog> moved by the animation library: no style element, only the CSSOM)
  await page.goto("/trash");
  await page.getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: "Empty trash" }).click();
  await page.getByRole("alertdialog", { name: "Empty the trash?" }).waitFor();
  await page.waitForTimeout(400);
  await page.keyboard.press("Escape");
  await page.getByRole("alertdialog").waitFor({ state: "detached" });

  // Upload: the file is hashed in a Web Worker with WebAssembly (hash-wasm), then the API is asked about it
  await page.goto("/library");
  await page.getByRole("button", { name: /Open IMG_00002/ }).first().waitFor();
  const checks: string[] = [];
  page.on("request", (r) => {
    if (new URL(r.url()).pathname === "/assets/check") checks.push(r.url());
  });
  await page.locator('input[type="file"][multiple]').first().setInputFiles({ name: "new.jpg", mimeType: "image/jpeg", buffer: Buffer.from("not really a jpeg") });
  await expect.poll(() => checks.length, { timeout: 15_000 }).toBeGreaterThan(0);

  expect(violations, "CSP violations while walking the app").toEqual([]);

  // The policy bites: a foreign image origin is refused, an allowed one is not.
  const outcome = await page.evaluate(
    ([foreign, allowed]) =>
      Promise.all(
        [foreign, allowed].map(
          (src) =>
            new Promise<string>((resolve) => {
              const done = (s: string) => resolve(s);
              document.addEventListener("securitypolicyviolation", (e) => e.blockedURI.startsWith(src!) && done("blocked"), { once: true });
              const img = new Image();
              img.onerror = () => setTimeout(() => done("error"), 300);
              img.onload = () => done("loaded");
              img.src = `${src}/fixture-thumbs/1.svg`;
            }),
        ),
      ),
    [FOREIGN, TEST_S3_ORIGIN],
  );
  expect(outcome[0]).toBe("blocked");
  expect(outcome[1]).toBe("loaded");
  violations.length = 0;
});
