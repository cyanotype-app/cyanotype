// People at phone and desktop widths: the way in (More menu), the People page with long and missing names and a hidden section, a
// person's page (a timeline, with suggestions), the inline rename, selection of the whole person and its dialogs, "Save as album", and the
// viewer's People chips, the faces on the photo and the "Who is this?" and "Is this X?" dialogs (the whole photo with the face outlined).
// Set SHOT_DIR to save the screenshots.
import { expect, test, type Locator } from "@playwright/test";
import { pageOf, timelineOf, type FakeAsset } from "./fakeLibrary";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

/** The outlined box of a decision dialog sits on the face: its place in the picture is the face's fractions of it, and all of it is on screen. */
async function expectBoxOnFace(dialog: Locator, box: [number, number, number, number]) {
  const img = dialog.locator(".face-context-frame img");
  await expect.poll(async () => (await img.boundingBox())?.width ?? 0).toBeGreaterThan(20);
  // The sheet slides in: both rectangles are read in one go, so they are of the same moment.
  const { i, b } = await dialog.evaluate((el) => {
    const r = (e: Element | null) => {
      const x = e!.getBoundingClientRect();
      return { x: x.x, y: x.y, width: x.width, height: x.height };
    };
    return { i: r(el.querySelector(".face-context-frame img")), b: r(el.querySelector("[data-testid=face-context-box]")) };
  });
  const tol = 1.5;
  expect(Math.abs(b.x - (i.x + box[0] * i.width)), "box left").toBeLessThan(tol);
  expect(Math.abs(b.y - (i.y + box[1] * i.height)), "box top").toBeLessThan(tol);
  expect(Math.abs(b.width - box[2] * i.width), "box width").toBeLessThan(tol);
  expect(Math.abs(b.height - box[3] * i.height), "box height").toBeLessThan(tol);
  const vp = dialog.page().viewportSize()!;
  const d = (await dialog.boundingBox())!;
  expect(d.x, "dialog left").toBeGreaterThanOrEqual(-0.5);
  expect(d.x + d.width, "dialog right").toBeLessThanOrEqual(vp.width + 0.5);
  // Every answer can be reached without leaving the screen (the dialog scrolls if the screen is short).
  for (const name of ["Not a face"]) await expect(dialog.getByRole("button", { name })).toBeVisible();
}

const LONG = "A really quite long name with_an_unbroken_run_of_characters_that_could_overflow_" + "x".repeat(10);
const cover = (n: number) => ({ face_id: `f${n}`, asset_id: `a${n}`, box: [0.35, 0.3, 0.3, 0.3], preview_url: `/fixture-thumbs/${n}.svg` });
const person = (id: string, name: string | null, photos: number, over: Record<string, unknown> = {}) => ({
  id,
  name,
  hidden: false,
  faces: photos,
  photos,
  cover: cover(Number(id.slice(1))),
  ...over,
});
const PEOPLE = [
  person("p1", LONG, 12345),
  person("p2", "Person B", 40),
  person("p3", null, 7),
  person("p4", "Person D", 0, { cover: null }),
  person("p5", "Person E", 3, { hidden: true }),
  person("p6", null, 2, { hidden: true }),
];

for (const { width, scheme } of RUNS) {
  test.describe(`people ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("People, a person, suggestions, selection, dialogs and the viewer's chips without overflow", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("people", width, scheme), problems);
      const calls: { method: string; path: string; body: unknown }[] = [];
      // The person's photos: the first twelve of the library (a timeline of their own, set once the library exists).
      let mine: FakeAsset[] = [];
      const handlers: Handler[] = [
        (route, url, method) => {
          const p = url.pathname;
          if (p === "/assets/people/status") {
            void route.fulfill(json({ enabled: true }));
            return true;
          }
          if (p === "/assets/people" && method === "GET") {
            void route.fulfill(json({ items: PEOPLE }));
            return true;
          }
          if (p === "/assets/people/p1" && method === "GET") {
            void route.fulfill(json(PEOPLE[0]));
            return true;
          }
          if (p === "/assets/people/p1" && method === "PATCH") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ ...PEOPLE[0], name: (route.request().postDataJSON() as { name?: string }).name ?? LONG }));
            return true;
          }
          if (p === "/assets/people/p1/suggestions") {
            void route.fulfill(
              json({
                items: [1, 2, 3, 4].map((n) => ({ face_id: `s${n}`, asset_id: `a${n}`, box: [0.3, 0.3, 0.4, 0.4], preview_url: `/fixture-thumbs/${n}.svg`, similarity: 0.6 })),
              }),
            );
            return true;
          }
          if (p === "/assets/people/p1/timeline") {
            void route.fulfill(json(timelineOf(mine, false)));
            return true;
          }
          if (p === "/assets/people/p1/photos") {
            const page = pageOf(mine, url.searchParams);
            void route.fulfill(json({ ...page, items: page.items.map((a) => ({ ...a, rating: 0, archived: false, hidden: false, edited: false })) }));
            return true;
          }
          if (p === "/assets/albums" && method === "GET") {
            const album = (id: string, title: string, count: number, thumb: number) => ({
              id,
              title,
              description: null,
              photo_count: count,
              cover: { id: `a${thumb}`, thumbhash: null, thumb_url: `/fixture-thumbs/${thumb}.svg` },
              first_at: "2024-01-01T00:00:00Z",
              last_at: "2024-06-30T00:00:00Z",
              created_at: "2024-01-01T00:00:00Z",
              updated_at: "2024-01-01T00:00:00Z",
            });
            void route.fulfill(json({ items: [album("l1", LONG, 30, 1), album("l2", "Pets", 12, 2), album("l3", "Garden", 7, 3), { ...album("l4", "Empty one", 0, 4), cover: null }] }));
            return true;
          }
          if (p === "/assets/albums" && method === "POST") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ id: "al1", title: "Holiday", description: null, photo_count: 0, cover: null, first_at: null, last_at: null, created_at: "", updated_at: "" }, 201));
            return true;
          }
          if (p === "/assets/albums/al1/assets" && method === "POST") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ count: 12345 }));
            return true;
          }
          if (p === "/assets/people/p1/faces/remove") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ removed: 1 }));
            return true;
          }
          if (/^\/assets\/a2\/faces$/.test(p)) {
            void route.fulfill(
              json({
                items: [
                  { id: "f-a", box: [0.1, 0.1, 0.2, 0.2], person: { id: "p1", name: LONG } },
                  { id: "f-b", box: [0.5, 0.1, 0.2, 0.2], person: { id: "p3", name: null } },
                  { id: "f-c", box: [0.1, 0.5, 0.2, 0.2], person: null },
                ],
                dismissed: [],
              }),
            );
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
      mine = assets.slice(0, 12);
      await page.route(/\/assets\/a2$/, (route) =>
        route.fulfill(json({ ...assets[2], albums: [], original_url: assets[2]!.thumb_url, preview_url: assets[2]!.thumb_url, tags: [] })),
      );

      // The People page.
      await page.goto("/people");
      await page.getByRole("list", { name: "People", exact: true }).waitFor();
      await page.getByRole("link", { name: /^Unnamed person, 7 photos/ }).waitFor();
      await check("people");
      await page.getByText("Hidden people (2)").click();
      await page.getByRole("list", { name: "Hidden people" }).waitFor();
      await check("people-hidden-section");

      // A person.
      await page.getByRole("link", { name: new RegExp(`^${LONG.slice(0, 20)}`) }).click();
      await page.getByRole("heading", { name: LONG, level: 1 }).waitFor();
      await page.getByRole("region", { name: "Suggestions" }).waitFor();
      await page.getByRole("button", { name: /Open IMG_00000/ }).waitFor();
      await page.getByRole("heading", { level: 2, name: /2026/ }).first().waitFor();
      await check("person");
      await check("person-menu-closed");
      await page.getByRole("button", { name: "More" }).click();
      await check("person-menu");
      await page.getByRole("menuitem", { name: "Rename" }).click();
      await page.getByLabel("Name", { exact: true }).fill("Person One");
      await check("person-rename");
      await page.getByRole("button", { name: "Save", exact: true }).click();
      await page.getByRole("heading", { name: "Person One", level: 1 }).waitFor();
      expect(calls.at(-1)?.body).toEqual({ name: "Person One" });

      // "Is this X?": the whole photo with the face outlined, answers from there.
      await page.getByRole("button", { name: "See the whole photo" }).first().click();
      const isThis = page.getByRole("dialog", { name: "Is this Person One?" });
      await isThis.waitFor();
      await expectBoxOnFace(isThis, [0.3, 0.3, 0.4, 0.4]);
      await expect(isThis.getByRole("button", { name: "Yes, this is Person One" })).toBeVisible();
      await expect(isThis.getByRole("button", { name: "No, this is not Person One" })).toBeVisible();
      await check("person-is-this");
      await page.keyboard.press("Escape");
      await isThis.waitFor({ state: "detached" });

      // Selecting photos and the "Not this person" question.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("person-select");
      await page.getByRole("button", { name: "Not this person" }).click();
      const notThis = page.getByRole("dialog", { name: /^Not / });
      await notThis.waitFor();
      await check("person-not-this-person");
      await notThis.getByRole("button", { name: "Not this person" }).click();
      await page.getByText(/^Took 2 photos out of/).waitFor();
      expect(calls.at(-1)?.body).toEqual({ asset_ids: ["a0", "a1"] });
      await check("person-removed-toast");

      // The whole person: the selection bar with every action, and "Save as album" through the scope.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: "Select all" }).click();
      await page.getByText("12 selected").waitFor();
      await check("person-select-all");
      await page.getByRole("toolbar", { name: "Selection" }).getByRole("button", { name: "Add to album" }).click();
      const add = page.getByRole("dialog", { name: "Add 12 photos to album" });
      await add.getByRole("button", { name: /Pets/ }).waitFor();
      await check("person-add-to-album");
      await add.getByLabel("Search albums").fill("pe");
      await check("person-add-to-album-search");
      // In a search field the first Escape clears the text (the browser's own behaviour); the second closes the sheet.
      await page.keyboard.press("Escape");
      await expect(add.getByLabel("Search albums")).toHaveValue("");
      await page.keyboard.press("Escape");
      await add.waitFor({ state: "detached" });
      await page.getByRole("toolbar", { name: "Selection" }).getByRole("button", { name: "Save as album" }).click();
      const save = page.getByRole("dialog", { name: "Save 12 photos as an album" });
      await save.waitFor();
      await check("person-save-album");
      await save.getByLabel("Album title").fill("Holiday");
      await save.getByRole("button", { name: "Create album" }).click();
      await page.getByText("Created Holiday with 12,345 photos").waitFor();
      expect(calls.at(-1)).toEqual({ method: "POST", path: "/assets/albums/al1/assets", body: { scope: { person: "p1" }, except: [] } });
      await check("person-saved-toast");
      await page.getByRole("button", { name: "Done" }).click();

      // Merge and delete dialogs.
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Merge into…" }).click();
      await page.getByRole("dialog", { name: /Merge/ }).waitFor();
      await page.getByRole("group", { name: "Merge into" }).waitFor();
      await check("person-merge");
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Delete person" }).click();
      await page.getByRole("dialog", { name: /^Delete / }).waitFor();
      await check("person-delete");
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Change cover" }).click();
      await page.getByRole("dialog", { name: /^Change cover/ }).waitFor();
      await check("person-cover");
      await page.keyboard.press("Escape");

      // The viewer: the faces on the photo (a click on one nobody named asks "Who is this?"), and the named people in Info.
      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_00002/ }).first().click();
      await page.getByRole("dialog", { name: "IMG_00002.jpg" }).waitFor();
      const unclaimed = page.getByRole("button", { name: "Unknown face: who is this?" });
      await expect(unclaimed).toHaveCount(2);
      await expect(page.getByRole("link", { name: `${LONG}: open their page` })).toHaveCount(1);
      await check("viewer-faces");
      await unclaimed.last().click();
      const who = page.getByRole("dialog", { name: "Who is this?" });
      await who.waitFor();
      await expectBoxOnFace(who, [0.1, 0.5, 0.2, 0.2]);
      await page.getByLabel("Person", { exact: true }).selectOption("p2");
      await check("viewer-who-is-this");
      await page.keyboard.press("Escape");
      await who.waitFor({ state: "detached" });
      await page.keyboard.press("i");
      const panel = page.locator("#details-panel");
      await panel.getByRole("link", { name: LONG }).waitFor();
      await expect(panel.getByText("Unknown face")).toHaveCount(0);
      await expect(panel.getByRole("link", { name: "Unnamed person" })).toHaveCount(0);
      await check("viewer-people");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
