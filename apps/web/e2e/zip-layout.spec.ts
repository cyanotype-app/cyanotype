// "Download as ZIP" at phone and desktop widths: the album header, the selection bar and the share page's "Download all" fit
// without overflow, and clicking them makes the real requests (a plan, then a form post / a plain link the browser saves).
// Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const ALBUM = {
  id: "z1",
  title: "Alpha trip along the entire Pacific coast highway with the whole extended family 2019",
  description: null,
  smart: false,
  rules: null,
  photo_count: 12,
  photo_count_capped: false,
  hidden_count: 0,
  cover: { id: "a1", thumbhash: null, thumb_url: "/fixture-thumbs/1.svg" },
  first_at: "2019-01-01T00:00:00Z",
  last_at: "2024-06-30T00:00:00Z",
  created_at: "2024-01-01T00:00:00Z",
  updated_at: "2024-01-01T00:00:00Z",
};

/** What the browser would save: an attachment, so the page stays where it is. */
const archive = {
  status: 200,
  contentType: "application/zip",
  headers: { "Content-Disposition": 'attachment; filename="Alpha.zip"', "Content-Length": "22" },
  body: Buffer.from("504b0506000000000000000000000000000000000000", "hex"),
};

for (const { width, scheme } of RUNS) {
  test.describe(`zip download ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("album header and selection bar", async ({ page }) => {
      test.setTimeout(90_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("zip", width, scheme), problems);
      const plans: unknown[] = [];
      const posts: string[] = [];
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          if (p === "/assets/zip/plan") {
            plans.push(JSON.parse(route.request().postData() ?? "null"));
            void route.fulfill(json({ files: 12, bytes: 36_000_000, name: "Alpha.zip" }));
            return true;
          }
          if (p === "/assets/zip" && method === "POST") {
            posts.push(route.request().postData() ?? "");
            void route.fulfill(archive);
            return true;
          }
          if (p === "/assets/albums" && method === "GET") {
            void route.fulfill(json({ items: [ALBUM] }));
            return true;
          }
          if (p === "/assets/albums/z1" && method === "GET") {
            void route.fulfill(json(ALBUM));
            return true;
          }
          if (p === "/assets/albums/z1/assets") {
            void route.fulfill(json({ items: [], next_cursor: null, next_before: null }));
            return true;
          }
          if (p === "/assets/search/facets") {
            void route.fulfill(json({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [] }));
            return true;
          }
          if (p === "/assets/tags" && method === "GET") {
            void route.fulfill(json({ items: [] }));
            return true;
          }
          return false;
        },
      ];
      const assets = await setupApp(page, { handlers });
      await page.route(/\/assets\/albums\/z1\/assets/, (route) =>
        route.fulfill(json({ items: assets.slice(0, 12), next_cursor: null, next_before: null })),
      );

      await page.goto("/albums/z1");
      await page.getByRole("heading", { name: /Alpha trip/ }).waitFor();
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      const download = page.getByRole("button", { name: "Download", exact: true });
      await expect(download).toBeVisible();
      await check("album-header");

      // The whole album: one request with the album scope and no ids; the page then posts the same selection as a form.
      await download.click();
      await expect(page.getByText(/Downloading 12 files \(34 MB\) as a ZIP/)).toBeVisible();
      expect(plans).toEqual([{ scope: { album: "z1" }, except: [], name: ALBUM.title }]);
      expect(posts).toHaveLength(1);
      expect(decodeURIComponent(posts[0]!.replace(/\+/g, " "))).toContain('"scope":{"album":"z1"}');
      await check("album-started");

      // A selection: the bar has its own Download, off until something is selected.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      const bar = page.getByRole("toolbar", { name: "Selection" });
      await expect(bar.getByRole("button", { name: "Download", exact: true })).toBeDisabled();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await expect(bar.getByRole("button", { name: "Download", exact: true })).toBeEnabled();
      await check("album-selection");
      await bar.getByRole("button", { name: "Download", exact: true }).click();
      await expect.poll(() => plans.length).toBe(2);
      expect(plans[1]).toMatchObject({ ids: ["a0", "a1"] });
      await expect.poll(() => posts.length).toBe(2);

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("share page: Download all", async ({ page }) => {
      test.setTimeout(90_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("zip-share", width, scheme), problems);
      const items = [1, 2, 3].map((n) => ({
        id: `p${n}`,
        width: 400,
        height: 300,
        sort_time: "2026-10-14T10:00:00Z",
        thumbhash: null,
        thumb_url: `/fixture-thumbs/${n}.svg`,
        preview_url: `/fixture-thumbs/${n}.svg`,
        downloads: [{ id: `p${n}`, label: "JPEG", size_bytes: null }],
        download_pending: false,
      }));
      let allowed = true;
      await page.route("**/assets/public/s/tok", (r) =>
        r.fulfill(
          json({
            needs_password: false,
            unlocked: true,
            kind: "album",
            title: ALBUM.title,
            item_count: 3,
            allow_download: allowed,
            include_originals: false,
            expires_at: null,
          }),
        ),
      );
      await page.route(/\/assets\/public\/s\/tok\/items(\?.*)?$/, (r) => r.fulfill(json({ items, next_cursor: null })));
      let asked = 0;
      await page.route("**/assets/public/s/tok/zip/plan", (r) => {
        asked++;
        // The first answer says one photo's copy is still being made; the page waits and asks again.
        r.fulfill(json({ files: asked === 1 ? 2 : 3, bytes: 3_000_000, pending: asked === 1 ? 1 : 0, skipped: 0, name: "Alpha.zip", max_files: 1000, max_bytes: 4 << 30 }));
      });
      // (A download started from a link is not routed by Playwright; the `download` event below proves the request.)

      await page.goto("/s/tok");
      const button = page.getByRole("button", { name: "Download all" });
      await expect(button).toBeVisible();
      await check("share-page");
      const saved = page.waitForEvent("download");
      await button.click();
      await expect(page.getByRole("status")).toContainText("Preparing the download");
      await check("share-preparing");
      // A real download (an attachment the browser saves), from the plain link.
      expect((await saved).url()).toMatch(/\/assets\/public\/s\/tok\/zip$/);
      expect(asked).toBe(2);
      await expect(page.getByRole("status")).toContainText("Downloading 3 files");
      await check("share-started");

      // Owner turned downloads off: no button.
      allowed = false;
      await page.reload();
      await page.getByRole("heading", { name: /Alpha trip/ }).waitFor();
      await expect(page.getByRole("button", { name: "Download all" })).toHaveCount(0);

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
