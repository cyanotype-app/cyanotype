// Renders every page and overlay with mocked API data at phone widths and asserts the page never scrolls
// sideways. Set SHOT_DIR to also save a screenshot of each state at every width.
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { installLibrary, makeAssets, pageOf, timelineOf, type FakeAsset } from "./fakeLibrary";

const SHOT_DIR = process.env.SHOT_DIR;
const RUNS: { width: number; scheme: "light" | "dark" }[] = [
  ...[320, 360, 375, 390, 414, 1280].map((width) => ({ width, scheme: "light" as const })),
  { width: 360, scheme: "dark" },
];
const LONG_ALBUM = "Summer road trip along the entire Pacific coast highway with the whole extended family, 2019 to 2024";
const LONG_NAME = "IMG_20240615_184233_a-really-quite-long-camera-generated-filename-with-no-spaces-at-all-HDR-PANO.jpg";

const palette = ["#1d4e7a", "#2f7cb5", "#5c9bcb", "#0a4a78", "#8fbbe0", "#14609a"];
const svg = (n: number) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300"><rect width="300" height="300" fill="${palette[n % palette.length]}"/><circle cx="150" cy="150" r="${40 + (n % 5) * 15}" fill="#fff" fill-opacity=".35"/></svg>`;
const thumb = (n: number) => `/fixture-thumbs/${n}.svg`;

const asset = (n: number) => ({
  id: `a${n}`,
  original_name: n === 2 ? LONG_NAME : `IMG_${1000 + n}.jpg`,
  content_type: "image/jpeg",
  size_bytes: 3_000_000,
  width: 4000,
  height: 3000,
  sort_time: new Date(Date.UTC(2024, 5, 30 - n, 12)).toISOString(),
  thumbhash: null,
  thumb_url: thumb(n),
});
const assets = Array.from({ length: 30 }, (_, i) => asset(i));
const trash = assets.slice(0, 9).map((a) => ({
  ...a,
  deleted_at: new Date(Date.now() - 2 * 86_400_000).toISOString(),
  purge_at: new Date(Date.now() + 28 * 86_400_000).toISOString(),
}));
const album = (id: string, title: string, count: number, withDesc = false) => ({
  id,
  title,
  description: withDesc ? "A long description. ".repeat(12) + "x".repeat(60) : null,
  photo_count: count,
  cover: { id: "a1", thumbhash: null, thumb_url: thumb(1) },
  first_at: "2019-01-01T00:00:00Z",
  last_at: "2024-06-30T00:00:00Z",
  created_at: "2024-01-01T00:00:00Z",
  updated_at: "2024-01-01T00:00:00Z",
});
const albums = [
  album("l1", LONG_ALBUM, 30, true),
  album("l2", "Pets", 12),
  album("l3", "Averyveryveryveryveryveryveryveryveryveryveryverylongsinglewordalbumtitle", 3),
  album("l4", "Garden", 7),
];

const IDLE = { active: false, queued: 0, running: 0, failed: 0, by_kind: [], total_photos: 30, without_thumbnail: 0, failures: [] };
const LONG_ERROR =
  "decode refused: original is 187 MB, which does not fit the worker memory budget of 640 MB (a-very-long-unbroken-error-token-" + "x".repeat(120) + ")";
const BUSY = {
  ...IDLE,
  active: true,
  queued: 41234,
  running: 2,
  total_photos: 52000,
  without_thumbnail: 20617,
  by_kind: [{ kind: "thumbnail", queued: 20617, running: 2, failed: 3 }],
  failed: 1234,
  failures: [
    { job_id: 1, asset_id: "a2", original_name: LONG_NAME, kind: "thumbnail", last_error: LONG_ERROR, attempts: 5 },
    { job_id: 2, asset_id: "a3", original_name: "IMG_1003.CR3", kind: "metadata", last_error: null, attempts: 1 },
    { job_id: 3, asset_id: "a4", original_name: "IMG_1004.HEIC", kind: "thumbnail", last_error: "sha256 mismatch", attempts: 5 },
  ],
};
let processing: typeof IDLE | typeof BUSY = IDLE;

async function mock(page: Page) {
  const json = (body: unknown) => ({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/fixture-thumbs/*.svg", (r) =>
    r.fulfill({ status: 200, contentType: "image/svg+xml", body: svg(Number(/(\d+)\.svg/.exec(r.request().url())![1])) }),
  );
  await page.route(
    (u) => /^\/(auth|assets)(\/|$)/.test(u.pathname),
    async (route) => {
      const req = route.request();
      const u = new URL(req.url());
      const p = u.pathname;
      const m = req.method();
      if (p === "/auth/me") return route.fulfill(json({ id: "u1", email: "someone.with.a.long.address@example-photo-library.org" }));
      if (p === "/auth/api-keys" && m === "GET")
        return route.fulfill(
          json({
            items: [
              { id: "k1", name: "laptop", prefix: "cy_abcd", created_at: "2024-06-01T00:00:00Z", last_used_at: null },
              { id: "k2", name: "A key with a really rather long descriptive name for the nightly import job", prefix: "cy_efgh", created_at: "2024-06-01T00:00:00Z", last_used_at: "2024-06-20T00:00:00Z" },
            ],
          }),
        );
      if (p === "/auth/api-keys" && m === "POST")
        return route.fulfill(
          json({ id: "k3", name: "new", prefix: "cy_zzzz", created_at: "2024-06-01T00:00:00Z", key: "cy_" + "0123456789abcdef".repeat(5) }),
        );
      if (p === "/assets/processing" && m === "GET") return route.fulfill(json(processing));
      if (p === "/assets/processing/retry" && m === "POST") {
        processing = { ...IDLE };
        return route.fulfill(json({ requeued: 1234 }));
      }
      if (p === "/assets/check") return route.fulfill({ status: 500, contentType: "application/json", body: '{"error":"fixture: upload refused"}' });
      if (p === "/assets/trash" && m === "GET")
        return route.fulfill(json({ items: trash, next_cursor: null, total: trash.length, retention_days: 30 }));
      if (p === "/assets/albums" && m === "GET") return route.fulfill(json({ items: albums }));
      const am = /^\/assets\/albums\/([^/]+)(\/assets)?$/.exec(p);
      if (am && m === "GET") {
        if (am[2]) return route.fulfill(json({ items: assets.slice(0, 18), next_cursor: null, next_before: null }));
        return route.fulfill(json(albums.find((a) => a.id === am[1]) ?? albums[0]));
      }
      if (p === "/assets/timeline" && m === "GET") return route.fulfill(json(timelineOf(assets as FakeAsset[], false)));
      if (p === "/assets" && m === "GET") return route.fulfill(json(pageOf(assets as FakeAsset[], u.searchParams)));
      const dm = /^\/assets\/(a\d+)$/.exec(p);
      if (dm && m === "GET") {
        const a = assets.find((x) => x.id === dm[1])!;
        return route.fulfill(
          json({
            ...a,
            albums: [
              { id: "l1", title: LONG_ALBUM },
              { id: "l2", title: "Pets" },
              { id: "l3", title: albums[2]!.title },
            ],
            original_url: thumb(0),
            preview_url: thumb(Number(a.id.slice(1))),
          }),
        );
      }
      if (m === "POST" || m === "PATCH" || m === "DELETE") return route.fulfill(json({ count: 1 }));
      return route.fulfill({ status: 404, contentType: "application/json", body: '{"error":"not mocked"}' });
    },
  );
}

for (const { width, scheme } of RUNS) {
  test.describe(`${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: { width, height: width === 1280 ? 800 : 740 }, hasTouch: width < 600 });

    test(`no horizontal overflow on any page or overlay`, async ({ page }) => {
      test.setTimeout(90_000);
      if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
      const problems: string[] = [];

      async function check(name: string) {
        await page.waitForTimeout(150);
        const r = await page.evaluate(() => {
          const w = window.innerWidth;
          const bad: string[] = [];
          for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
            const b = el.getBoundingClientRect();
            if (b.width === 0 || el.closest(".tile")) continue; // tiles clip their hover-zoomed image
            if (b.right > w + 0.5 || b.left < -0.5) {
              bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} "${(el.textContent ?? "").trim().slice(0, 24)}" [${Math.round(b.left)}..${Math.round(b.right)}]`);
            }
          }
          return { scroll: document.documentElement.scrollWidth, w, bad: bad.slice(0, 6) };
        });
        if (r.scroll > r.w || r.bad.length) problems.push(`${name}: scrollWidth ${r.scroll} > ${r.w}; offenders: ${r.bad.join("; ")}`);
        if (SHOT_DIR) await page.screenshot({ path: join(SHOT_DIR, `${width}${scheme === "dark" ? "-dark" : ""}-${name}.png`) });
      }
      const open = async (path: string, ready: string | RegExp) => {
        await page.goto(path);
        await (typeof ready === "string" ? page.getByText(ready) : page.getByRole("button", { name: ready })).first().waitFor();
        await page.waitForLoadState("networkidle");
      };

      await mock(page);

      // Auth
      await page.route("**/auth/me", (r) => r.fulfill({ status: 401, contentType: "application/json", body: '{"error":"no"}' }));
      await open("/login", "Sign in");
      await check("login");
      await open("/register", "Create");
      await check("register");
      await page.unroute("**/auth/me");

      // Library
      await open("/library", /Open IMG_1000/);
      await check("library");
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_1000/ }).click();
      await page.getByRole("button", { name: /Select IMG_1001/ }).click();
      await check("library-select");
      await page.getByRole("button", { name: "Select all" }).click();
      await check("library-select-all");
      await page.getByRole("button", { name: "Add to album" }).click();
      await page.getByRole("dialog").waitFor();
      await check("dialog-add-to-album");
      await page.getByRole("button", { name: /new album/i }).click().catch(() => undefined);
      await check("dialog-add-to-album-new");
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Move to trash" }).last().click();
      await page.getByText("moved to trash").waitFor();
      await check("library-toast-undo");

      // Processing indicator: banner while photos are processed, the failures panel, and after a retry
      processing = BUSY;
      await open("/library", /Open IMG_1000/);
      await page.getByText("Processing 20,617 photos…").waitFor();
      await check("library-processing");
      await page.getByRole("button", { name: "1,234 failed" }).click();
      await page.getByRole("button", { name: "Retry failed" }).waitFor();
      await check("library-processing-panel");
      await page.getByRole("button", { name: "Retry failed" }).click();
      await page.getByText("Processing 20,617 photos…").waitFor({ state: "detached", timeout: 10_000 });
      await check("library-processing-done");
      processing = IDLE;

      // Lightbox
      await open("/library", /Open IMG_1000/);
      await page.getByRole("button", { name: /Open IMG_20240615/ }).click();
      await page.getByText("In albums:").waitFor();
      await check("lightbox-long-name");
      await page.getByRole("button", { name: "Next photo" }).click();
      await check("lightbox");

      // Upload tray (the mocked check endpoint refuses, so every file shows as failed)
      await open("/library", /Open IMG_1000/);
      await page.locator('input[aria-label="Choose photos"]').setInputFiles([
        { name: LONG_NAME, mimeType: "image/jpeg", buffer: Buffer.from("x") },
        { name: "second.jpg", mimeType: "image/jpeg", buffer: Buffer.from("y") },
      ]);
      await page.getByRole("region", { name: "Uploads" }).waitFor();
      await page.getByText(/Failed/).first().waitFor();
      await check("upload-tray");
      await page.getByRole("button", { name: "Hide" }).click();
      await check("upload-tray-collapsed");

      // Trash
      await open("/trash", "Trash");
      await page.getByRole("button", { name: /Select IMG_1000/ }).waitFor();
      await check("trash");
      await page.getByRole("button", { name: /Select IMG_1001/ }).click();
      await check("trash-select");
      await page.getByRole("button", { name: "Delete forever" }).click();
      await check("dialog-delete-forever");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "Empty trash" }).click();
      await check("dialog-empty-trash");
      await page.getByRole("button", { name: "Cancel" }).click();

      // Albums
      await open("/albums", "Albums");
      await page.getByRole("link", { name: /Pets/ }).waitFor();
      await check("albums");
      await page.getByRole("button", { name: "New album" }).click();
      await check("dialog-new-album");
      await page.keyboard.press("Escape");

      // Album page
      await open("/albums/l1", "Edit");
      await page.getByRole("button", { name: /Open IMG_1000/ }).waitFor();
      await check("album");
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await check("album-edit");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_1001/ }).click();
      await check("album-select");
      await page.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: "Delete album" }).click();
      await check("dialog-delete-album");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "Add photos" }).click();
      await page.getByRole("button", { name: /Select IMG_1000/ }).waitFor();
      await page.getByRole("button", { name: /Select IMG_1003/ }).click();
      await check("album-photo-picker");
      await page.getByRole("dialog", { name: "Add photos" }).getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: /Open IMG_1000/ }).click();
      await check("album-lightbox");

      // Timeline of a larger library: month and day headers, the scrubber (resting and dragged), hearts, downloads.
      await installLibrary(page, makeAssets(4000), (r) => r.fallback());
      await open("/library", /Open IMG_00000/);
      await page.locator(".scrubber").waitFor();
      await check("timeline");
      const handle = (await page.getByRole("slider").boundingBox())!;
      const rail = (await page.locator(".scrubber").boundingBox())!;
      await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
      await page.mouse.down();
      await page.mouse.move(handle.x + handle.width / 2, rail.y + rail.height * 0.55, { steps: 6 });
      await page.locator(".scrubber-bubble").waitFor();
      await check("timeline-scrubber-drag");
      await page.mouse.up();
      await page.locator(".tile-cell img").first().waitFor();
      await check("timeline-scrolled");
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("timeline-select");
      await page.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: /Open IMG_00000/ }).click(); // a RAW + JPEG pair
      await page.getByRole("dialog").getByRole("button", { name: "Download" }).click();
      await page.getByRole("link", { name: /Download RAW/ }).waitFor();
      await check("lightbox-download-pair");
      await page.getByRole("button", { name: "Add to favorites" }).click();
      await check("lightbox-favorited");
      await page.keyboard.press("Escape");
      await open("/favorites", /Open IMG_00003/);
      await check("favorites-view");

      // API keys
      await open("/settings/api-keys", "API keys");
      await page.getByText("nightly import").waitFor();
      await page.getByLabel("Name").fill("new");
      await page.getByRole("button", { name: "Create key" }).click();
      await page.getByText("Copy your new key").waitFor();
      await check("api-keys");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
