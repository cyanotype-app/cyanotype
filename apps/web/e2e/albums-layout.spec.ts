// Albums and sharing at phone and desktop widths: the Albums grid (cover, badges, long and missing titles), the new-album sheet, an
// album page (the cover hero, its menu, edit details, selection, the photo picker), the share sheet and its link with a QR code,
// and the Shared links list with its edit and revoke sheets. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG = "Summer road trip along the entire Pacific coast highway with the whole extended family, 2019 to 2024";
const DESC = "A long description. ".repeat(10);
const WORD = "Averyveryveryveryveryveryveryveryveryveryveryverylongsinglewordalbumtitle";
const album = (id: string, title: string, count: number, over: Record<string, unknown> = {}) => ({
  id,
  title,
  description: null,
  hidden_count: 0,
  photo_count: count,
  cover: { id: "a1", thumbhash: null, thumb_url: `/fixture-thumbs/${(Number(id.slice(1)) % 6) + 1}.svg` },
  first_at: "2019-01-01T00:00:00Z",
  last_at: "2024-06-30T00:00:00Z",
  created_at: "2024-01-01T00:00:00Z",
  updated_at: "2024-01-01T00:00:00Z",
  ...over,
});
const ALBUMS = [
  album("l1", LONG, 30, { description: DESC }),
  album("l2", "Pets", 12),
  album("l3", WORD, 3),
  album("l4", "Garden", 0, { cover: null, first_at: null, last_at: null }),
  album("l5", "Faves", 10000, { smart: true, photo_count_capped: true, rules: { match: "all", archived: "exclude", rules: [{ field: "favorite", value: true }] } }),
  album("l6", "Winter", 7),
];
const share = (n: number, over: Record<string, unknown> = {}) => ({
  id: `s${n}`,
  kind: "selection",
  album: null,
  title: null,
  token: `tok${n}`.padEnd(32, "x"),
  has_password: false,
  allow_download: true,
  include_originals: false,
  expires_at: null,
  expired: false,
  item_count: 4,
  view_count: 2,
  last_viewed_at: null,
  created_at: "2024-06-01T00:00:00Z",
  updated_at: "2024-06-01T00:00:00Z",
  ...over,
});
const SHARES = [
  share(1, { kind: "album", album: { id: "l1", title: LONG }, item_count: 30, has_password: true, expires_at: new Date(Date.now() + 3 * 86_400_000).toISOString() }),
  share(2, { title: "For the neighbours", expired: true, expires_at: "2024-01-01T00:00:00Z", include_originals: true }),
  share(3, { title: LONG, allow_download: false, item_count: 1 }),
];

for (const { width, scheme } of RUNS) {
  test.describe(`albums ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("grid, album page, sheets, sharing and Shared links without overflow", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("albums", width, scheme), problems);
      const calls: { method: string; path: string; body: unknown }[] = [];
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          if (p === "/assets/albums" && method === "GET") {
            void route.fulfill(json({ items: ALBUMS }));
            return true;
          }
          if (p === "/assets/albums" && method === "POST") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json(album("l9", "Fresh", 0, { cover: null }), 201));
            return true;
          }
          if (p === "/assets/albums/l1" && method === "GET") {
            void route.fulfill(json(ALBUMS[0]));
            return true;
          }
          if (p === "/assets/albums/l9" && method === "GET") {
            void route.fulfill(json(album("l9", "Fresh", 0, { cover: null, first_at: null, last_at: null })));
            return true;
          }
          if (p === "/assets/albums/l9/assets" && method === "GET") {
            void route.fulfill(json({ items: [], next_cursor: null, next_before: null }));
            return true;
          }
          if (p === "/assets/albums/l1" && method === "PATCH") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ ...ALBUMS[0], ...(route.request().postDataJSON() as object) }));
            return true;
          }
          if (p === "/assets/shares" && method === "GET") {
            void route.fulfill(json({ items: SHARES }));
            return true;
          }
          if (p === "/assets/shares" && method === "POST") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json(share(9, { kind: "album", album: { id: "l1", title: LONG }, item_count: 30 }), 201));
            return true;
          }
          if (p === "/assets/search/facets") {
            void route.fulfill(json({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [] }));
            return true;
          }
          return false;
        },
      ];
      const assets = await setupApp(page, { handlers });
      await page.route(/\/assets\/albums\/l1\/assets/, (route) => route.fulfill(json({ items: assets.slice(0, 18), next_cursor: null, next_before: null })));

      // The grid.
      await page.goto("/albums");
      await page.getByRole("list", { name: "Albums" }).waitFor();
      await page.getByRole("link", { name: /Pets/ }).waitFor();
      await expect(page.getByRole("list", { name: "Albums" }).getByText("Shared", { exact: true })).toBeVisible();
      await check("albums");
      await page.getByRole("button", { name: "New", exact: true }).click();
      await check("albums-new-menu");
      await page.getByRole("menuitem", { name: "New album" }).click();
      await page.getByRole("dialog", { name: "New album" }).waitFor();
      await page.getByLabel("Album title").fill(LONG);
      await check("albums-new-sheet");
      await page.keyboard.press("Escape");
      await page.getByRole("dialog").waitFor({ state: "detached" });

      // Create one: it opens empty, with a way to add photos.
      await page.getByRole("button", { name: "New", exact: true }).click();
      await page.getByRole("menuitem", { name: "New album" }).click();
      await page.getByLabel("Album title").fill("Fresh");
      await page.getByRole("button", { name: "Create", exact: true }).click();
      await page.getByRole("heading", { name: "Fresh", level: 1 }).waitFor();
      await page.getByText("This album is empty").waitFor();
      expect(calls.at(-1)?.body).toMatchObject({ title: "Fresh" });
      await check("album-empty");

      // An album with a long title and description.
      await page.goto("/albums/l1");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await check("album");
      await page.getByRole("button", { name: "More", exact: true }).click();
      await check("album-menu");
      await page.getByRole("menuitem", { name: "Edit details" }).click();
      await page.getByRole("dialog", { name: "Edit details" }).waitFor();
      await check("album-edit");
      await page.getByLabel("Title", { exact: true }).fill("Road trip");
      await page.getByRole("button", { name: "Save", exact: true }).click();
      await page.getByRole("heading", { name: "Road trip", level: 1 }).waitFor();
      expect(calls.at(-1)?.body).toEqual({ title: "Road trip", description: DESC.trim() });
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("album-select");
      await page.getByRole("button", { name: "Done" }).click();

      // The photo picker.
      await page.getByRole("button", { name: "Add photos" }).first().click();
      const picker = page.getByRole("dialog", { name: "Add photos" });
      await picker.getByRole("button", { name: /Select IMG_00003/ }).click();
      await check("album-photo-picker");
      await page.keyboard.press("Escape");
      await picker.waitFor({ state: "detached" });

      // Sharing: the sheet, then the link with its QR code.
      await page.getByRole("button", { name: "Share", exact: true }).first().click();
      const sheet = page.getByRole("dialog", { name: /^Share the album/ });
      await sheet.waitFor();
      await check("share-sheet");
      await sheet.getByLabel("Password (optional)").fill("a-fairly-long-password-to-type");
      await sheet.getByLabel(/Include originals/).click();
      await check("share-sheet-originals");
      await sheet.getByRole("button", { name: "Create link" }).click();
      await page.getByRole("heading", { name: "Link created" }).waitFor();
      await page.getByRole("img", { name: "QR code of the link" }).waitFor();
      await check("share-created");
      await page.getByRole("button", { name: "Done" }).click();

      // Shared links.
      await page.goto("/shared");
      await page.getByRole("list", { name: "Shared links" }).waitFor();
      await check("shared-links");
      await page.getByRole("button", { name: "Edit" }).first().click();
      await page.getByRole("dialog", { name: "Edit link" }).waitFor();
      await check("shared-edit");
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Revoke" }).first().click();
      await page.getByRole("dialog", { name: "Revoke this link?" }).waitFor();
      await check("shared-revoke");
      await page.keyboard.press("Escape");

      // Delete asks first.
      await page.goto("/albums/l1");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByRole("button", { name: "More", exact: true }).click();
      await page.getByRole("menuitem", { name: "Delete album" }).click();
      await page.getByRole("dialog", { name: "Delete this album?" }).waitFor();
      await check("album-delete");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
