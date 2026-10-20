// Near-duplicate review at phone and desktop widths: groups of two to four photos side by side with long names and every kind of metadata,
// the empty and "checking" states, and the lightbox over a group. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, LONG_NAME, type Handler } from "./kit";

const member = (n: number, over: Record<string, unknown> = {}) => ({
  id: `a${n}`,
  original_name: n === 1 ? LONG_NAME : `IMG_${1000 + n}.jpg`,
  content_type: "image/jpeg",
  size_bytes: n === 1 ? 5_242_880 : 90_000 * n,
  width: n === 1 ? 6000 : 6000 / (n + 1),
  height: n === 1 ? 4000 : 4000 / (n + 1),
  sort_time: "2024-06-15T18:42:33Z",
  thumbhash: null,
  thumb_url: `/fixture-thumbs/${n}.svg`,
  favorite: n === 2,
  rating: n === 2 ? 4 : 0,
  archived: false,
  hidden: false,
  exif_fields: n === 1 ? 48 : n === 4 ? 0 : 12,
  has_location: n !== 3,
  has_caption: n === 2,
  tag_count: n === 2 ? 3 : 0,
  best: n === 1,
  ...over,
});
const GROUPS = [
  { id: "g1", members: [member(1), member(2), member(3), member(4)] },
  { id: "g2", members: [member(5), member(6)] },
];
const done = { state: "idle", groups: 2, photos: 12345, finished_at: new Date().toISOString() };

for (const { width, scheme } of RUNS) {
  test.describe(`duplicates ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("review, empty and checking states without overflow", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("a4", width, scheme), problems);
      let phase: "groups" | "empty" | "running" = "groups";
      const kept: unknown[] = [];
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          if (p === "/assets/duplicates" && method === "GET") {
            const body =
              phase === "groups"
                ? { groups: GROUPS, total: 2, next_cursor: null, scan: done }
                : phase === "empty"
                  ? { groups: [], total: 0, next_cursor: null, scan: { ...done, groups: 0 } }
                  : { groups: [], total: 0, next_cursor: null, scan: { state: "running", started_at: new Date().toISOString() } };
            void route.fulfill(json(body));
            return true;
          }
          if (/^\/assets\/duplicates\/g\d\/keep$/.test(p)) {
            kept.push(route.request().postDataJSON());
            void route.fulfill(json({ kept: "a1", trashed: ["a2", "a3", "a4"], merged: { albums: 2, tags: 3, favorite: true, rating: 4, caption: true, location: true } }));
            return true;
          }
          if (p === "/assets/duplicates/scan") {
            phase = "running";
            void route.fulfill(json({ scan: { state: "wanted", manual: true } }));
            return true;
          }
          return false;
        },
      ];
      const assets = await setupApp(page, { handlers });
      await page.route(/\/assets\/a\d$/, (route) => {
        const n = Number(/a(\d)$/.exec(route.request().url())![1]);
        return route.fulfill(json({ ...assets[n % 10]!, albums: [], original_url: `/fixture-thumbs/${n}.svg`, preview_url: `/fixture-thumbs/${n}.svg` }));
      });

      await page.goto("/duplicates");
      await page.getByRole("heading", { name: "Group 1 · 4 photos" }).waitFor();
      await check("groups");
      await page.getByRole("button", { name: /Look at IMG_1002/ }).click();
      await page.getByRole("dialog").waitFor();
      await check("group-lightbox");
      await page.keyboard.press("Escape");
      await page.getByRole("radio").nth(1).check();
      await check("second-chosen");
      await page.getByRole("button", { name: /Keep selected, trash 3 others/ }).click();
      await page.getByText(/3 photos moved to the trash/).waitFor();
      expect(kept.at(-1)).toEqual({ keep: "a2" });
      await check("kept-toast");

      phase = "empty";
      await page.goto("/duplicates");
      await page.getByRole("heading", { name: "No duplicates to review" }).waitFor();
      await check("empty");
      await page.getByRole("button", { name: "Find duplicates" }).click();
      await page.getByRole("button", { name: "Checking…" }).waitFor();
      await check("checking");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
