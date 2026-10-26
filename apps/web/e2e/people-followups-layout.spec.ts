// The People follow-ups at phone and desktop widths: the People page in its three sections (named, unnamed, hidden), the badge of
// suggestions waiting, selecting people with the bar and its merge and delete dialogs, and a person's "Change cover" picker. Set
// SHOT_DIR to save the screenshots.
import { expect, test, type Locator } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG = "A really quite long name with_an_unbroken_run_of_characters_that_could_overflow_" + "x".repeat(10);
const cover = (n: number) => ({ face_id: `f${n}`, asset_id: `a${n}`, box: [0.35, 0.3, 0.3, 0.3], preview_url: `/fixture-thumbs/${n}.svg` });
const person = (id: string, name: string | null, photos: number, over: Record<string, unknown> = {}) => ({
  id,
  name,
  hidden: false,
  faces: photos,
  photos,
  pending: 0,
  cover: cover(Number(id.slice(1))),
  ...over,
});
let PEOPLE = [
  person("p1", LONG, 12345, { pending: 3 }),
  person("p2", "Person B", 40, { pending: 120 }),
  person("p3", "Person C", 12, { pending: 1 }),
  person("p4", "Person D", 0, { cover: null }),
  person("p5", null, 7),
  person("p6", null, 2),
  person("p7", "Person G", 3, { hidden: true }),
  person("p8", null, 2, { hidden: true }),
];
const FACES = Array.from({ length: 30 }, (_, i) => ({ face_id: `cf${i}`, asset_id: `a${i}`, box: [0.3, 0.3, 0.4, 0.4], preview_url: `/fixture-thumbs/${(i % 6) + 1}.svg` }));

for (const { width, scheme } of RUNS) {
  test.describe(`people follow-ups ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("sections, badge, selection bar, merge and delete dialogs and the cover picker without overflow", async ({ page }) => {
      test.setTimeout(150_000);
      PEOPLE = PEOPLE.map((p) => ({ ...p }));
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("people-followups", width, scheme), problems);
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
          if (p === "/assets/people/bulk" && method === "POST") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ done: 2, into: "p1", faces: 5 }));
            return true;
          }
          if (p === "/assets/people/p1" && method === "GET") {
            void route.fulfill(json(PEOPLE[0]));
            return true;
          }
          if (p === "/assets/people/p1" && method === "PATCH") {
            calls.push({ method, path: p, body: route.request().postDataJSON() });
            void route.fulfill(json({ ...PEOPLE[0], cover: cover(4) }));
            return true;
          }
          if (p === "/assets/people/p1/faces") {
            const from = Number(url.searchParams.get("cursor") ?? 0);
            const items = FACES.slice(from, from + 24);
            void route.fulfill(json({ items, next_cursor: from + 24 < FACES.length ? String(from + 24) : null }));
            return true;
          }
          if (p === "/assets/people/p1/suggestions") {
            void route.fulfill(json({ items: [] }));
            return true;
          }
          if (p === "/assets/people/p1/timeline") {
            void route.fulfill(json({ total: 0, days: [] }));
            return true;
          }
          if (p === "/assets/people/p1/photos") {
            void route.fulfill(json({ items: [], next_cursor: null, next_before: null }));
            return true;
          }
          if (p === "/assets/search/facets") {
            void route.fulfill(json({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [] }));
            return true;
          }
          return false;
        },
      ];
      await setupApp(page, { handlers });

      // The People page: named, then Unnamed, then the collapsed Hidden section; the badges.
      await page.goto("/people");
      await page.getByRole("list", { name: "People", exact: true }).waitFor();
      await page.getByRole("region", { name: /^Unnamed/ }).waitFor();
      await page.getByRole("link", { name: /3 photos to confirm$/ }).waitFor();
      expect(await page.getByRole("link", { name: /1 photo to confirm$/ }).count()).toBe(1);
      expect(await page.locator("[data-badge]").allTextContents()).toEqual(["3", "99+", "1"]);
      const top = (l: Locator) => l.evaluate((e) => e.getBoundingClientRect().top);
      const [named, unnamed, hidden] = [page.getByRole("list", { name: "People", exact: true }), page.getByRole("region", { name: /^Unnamed/ }), page.locator("details[data-hidden-people]")];
      expect(await top(named)).toBeLessThan(await top(unnamed));
      expect(await top(unnamed)).toBeLessThan(await top(hidden));
      await check("sections");
      await page.getByText("Hidden people (2)").click();
      await check("hidden-section");
      await page.getByText("Hidden people (2)").click();

      // Selecting people: the bar, the unnamed and the long name.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      const bar = page.getByRole("toolbar", { name: "Selection" });
      await bar.getByText("Select people").waitFor();
      await check("select-empty");
      await page.getByRole("button", { name: new RegExp(`^${LONG.slice(0, 20)}`) }).click();
      await page.getByRole("button", { name: /^Unnamed person, 7 photos/ }).click();
      await bar.getByText("2 selected").waitFor();
      await check("select-two");

      // Merge: choose who stays, name them.
      await bar.getByRole("button", { name: "Merge" }).click();
      const merge = page.getByRole("dialog", { name: "Merge 2 people into one?" });
      await merge.waitFor();
      await expect(merge.getByRole("radio", { name: new RegExp(LONG.slice(0, 20)) })).toBeChecked();
      await merge.getByLabel("Name (optional)").fill(LONG);
      await check("merge-dialog");
      await merge.getByRole("button", { name: "Merge" }).click();
      await page.getByText(/^Merged 2 people into/).waitFor();
      expect(calls.at(-1)?.body).toEqual({ action: "merge", ids: ["p1", "p5"], into: "p1", name: LONG });
      await check("merged-toast");

      // Delete asks first; hidden people can be selected too (and shown again).
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: "Select all" }).click();
      await page.getByText("8 selected").waitFor();
      await check("select-all");
      await bar.getByRole("button", { name: "Delete" }).click();
      const del = page.getByRole("dialog", { name: "Delete 8 people?" });
      await del.waitFor();
      await check("delete-dialog");
      await page.keyboard.press("Escape");
      await expect(del).toHaveCount(0);
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await expect(bar).toHaveCount(0);

      // A person's page: Change cover, a page of faces, Show more, choosing one.
      await page.goto("/people/p1");
      await page.getByRole("heading", { name: LONG, level: 1 }).waitFor();
      await check("person-header");
      await page.getByRole("button", { name: "Change cover" }).click();
      const picker = page.getByRole("dialog", { name: /^Change cover of/ });
      await picker.getByRole("button", { name: "Face 24" }).waitFor();
      await check("cover-picker");
      await picker.getByRole("button", { name: "Show more" }).click();
      await picker.getByRole("button", { name: "Face 30" }).waitFor();
      await expect(picker.getByRole("button", { name: "Face 25" })).toBeFocused();
      await check("cover-picker-more");
      await picker.getByRole("button", { name: "Face 5" }).click();
      await page.getByText("Cover changed").waitFor();
      expect(calls.at(-1)?.body).toEqual({ cover_face_id: "cf4" });
      await expect(picker).toHaveCount(0);
      await check("cover-changed");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
