// People at phone and desktop widths: the way in (More menu), the People page with long and missing names and a hidden section, a
// person's page with suggestions, the inline rename, selection and its dialogs, and the viewer's People chips and "Who is this?"
// dialog. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

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
          if (p === "/assets/people/p1/photos") {
            void route.fulfill(json({ items: [], next_cursor: null, next_before: null }));
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
      // The person's photos: the first six of the library (set after setupApp so its handler order is kept: ours answer first).
      await page.route("**/assets/people/p1/photos*", (route) =>
        route.fulfill(json({ items: assets.slice(0, 6).map((a) => ({ ...a, rating: 0, archived: false, hidden: false, edited: false })), next_cursor: null, next_before: null })),
      );
      await page.route(/\/assets\/a2$/, (route) =>
        route.fulfill(json({ ...assets[2], albums: [], original_url: assets[2]!.thumb_url, preview_url: assets[2]!.thumb_url, tags: [] })),
      );

      // The way in: the More menu of the library.
      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByLabel("More views").click();
      await page.getByRole("link", { name: "People", exact: true }).waitFor();
      await check("more-menu");
      await page.getByRole("link", { name: "People", exact: true }).click();

      // The People page.
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
      await check("person");
      await page.getByRole("button", { name: "Rename" }).click();
      await page.getByLabel("Name", { exact: true }).fill("Person One");
      await check("person-rename");
      await page.getByRole("button", { name: "Save" }).click();
      await page.getByRole("heading", { name: "Person One", level: 1 }).waitFor();
      expect(calls.at(-1)?.body).toEqual({ name: "Person One" });

      // Selecting photos and the "Not this person" question.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("person-select");
      await page.getByRole("button", { name: "Not this person" }).click();
      await page.getByRole("alertdialog").waitFor();
      await check("person-not-this-person");
      await page.getByRole("alertdialog").getByRole("button", { name: "Not this person" }).click();
      await page.getByText(/^Took 2 photos out of/).waitFor();
      expect(calls.at(-1)?.body).toEqual({ asset_ids: ["a0", "a1"] });
      await check("person-removed-toast");

      // Merge and delete dialogs.
      await page.getByRole("button", { name: "Merge into…" }).click();
      await page.getByRole("dialog", { name: /Merge/ }).waitFor();
      await page.getByLabel("Merge into").waitFor();
      await check("person-merge");
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Delete person" }).click();
      await page.getByRole("alertdialog").waitFor();
      await check("person-delete");
      await page.keyboard.press("Escape");

      // The viewer: the people in the photo, and naming a face nobody has claimed.
      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_00002/ }).first().click();
      await page.getByRole("dialog", { name: "IMG_00002.jpg" }).waitFor();
      await page.keyboard.press("i");
      const panel = page.getByRole("complementary", { name: "Photo details" });
      await panel.getByRole("button", { name: "Name this face" }).waitFor();
      await check("viewer-people");
      await panel.getByRole("button", { name: "Name this face" }).click();
      await page.getByRole("dialog", { name: "Who is this?" }).waitFor();
      await page.getByLabel("Person", { exact: true }).selectOption("p2");
      await check("viewer-who-is-this");
      await page.keyboard.press("Escape");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
