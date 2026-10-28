// Collections without a card per kind (the Pets row is only the pets the owner named), and a dog the detector called a cat: put in a dog pet
// from the viewer's Info panel and from a selection (the animal is asked about, marked as a cat, and becomes a dog), and "It's a dog" in the
// viewer's Pets row. Set SHOT_DIR to save the screenshots.
import { expect, test, type Page } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const box = [0.35, 0.3, 0.3, 0.3];
const SUMMARY = {
  any: 9,
  kinds: [
    { kind: "dog", photos: 6, cover: { asset_id: "a0", box, preview_url: "/fixture-thumbs/0.svg" } },
    { kind: "cat", photos: 3, cover: { asset_id: "a6", box, preview_url: "/fixture-thumbs/6.svg" } },
  ],
  scanned: 30,
  total: 30,
};

interface Call {
  method: string;
  path: string;
  body: unknown;
}

const named = (id: string, name: string, kind: "dog" | "cat", photos: number, hidden = false) => ({
  id,
  name,
  kind,
  hidden,
  photos,
  detections: photos,
  pending: 0,
  cover: { detection_id: `d-${id}`, asset_id: "a0", box, preview_url: "/fixture-thumbs/0.svg" },
});

/** The Pets routes of a library whose photo a2 holds one animal the detector called a cat (`x1`); Biscuit is a dog. */
function backend(pets: ReturnType<typeof named>[]) {
  const calls: Call[] = [];
  const state = { kind: "cat" as "dog" | "cat", pet: null as { id: string; name: string } | null };
  const handler: Handler = (route, url, method) => {
    const p = url.pathname;
    const ok = (body: unknown, status = 200) => {
      void route.fulfill(json(body, status));
      return true;
    };
    const record = () => {
      const body = method === "GET" ? null : route.request().postDataJSON();
      calls.push({ method, path: p, body });
      return body as Record<string, unknown> | null;
    };
    if (p === "/assets/pets/status") return ok({ enabled: true });
    if (p === "/assets/pets") return ok(SUMMARY);
    if (p === "/assets/pets/named" && method === "GET") return ok({ items: pets });
    if (p === "/assets/pets/named/p1/photos" && method === "POST") {
      const b = record() as { asset_ids?: string[]; detection_ids?: string[] };
      const pet = pets[0]!;
      if (b.detection_ids) {
        state.kind = pet.kind;
        state.pet = { id: pet.id, name: pet.name };
        return ok({ pet, assigned: b.detection_ids.length, ambiguous: [], skipped: [] });
      }
      // The photo has no dog, but it has an animal the detector called a cat: the owner is asked.
      return ok({
        pet,
        assigned: 0,
        ambiguous: [{ asset_id: "a2", preview_url: "/fixture-thumbs/2.svg", candidates: [{ detection_id: "x1", kind: "cat", box: [0.3, 0.3, 0.4, 0.4] }] }],
        skipped: [],
      });
    }
    if (p === "/assets/pets/detections/x1/kind") {
      const b = record() as { kind: "dog" | "cat" };
      state.kind = b.kind;
      state.pet = null;
      return ok({ id: "x1", kind: b.kind });
    }
    if (p === "/assets/a2/pets") return ok({ items: [{ id: "x1", kind: state.kind, box: [0.3, 0.3, 0.4, 0.4], score: 0.9, pet: state.pet }], scanned: true });
    const animals = /^\/assets\/(a\d+)\/pets$/.exec(p);
    if (animals) return ok({ items: [], scanned: true });
    return false;
  };
  return { handler, calls, last: () => calls.at(-1) };
}

const open = async (page: Page, name: RegExp) => {
  await page.getByRole("button", { name }).first().click();
  await page.getByRole("dialog", { name: /IMG_00002/ }).waitFor();
  await page.keyboard.press("i");
  return page.locator("#details-panel");
};

for (const { width, scheme } of RUNS.filter((r) => r.width === 390 || r.width === 1280 || r.scheme === "dark")) {
  test.describe(`pet kind ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("Collections shows the pets the owner named and no card per kind", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("petkind-collections", width, scheme), problems);
      const fake = backend([named("p1", "Biscuit", "dog", 6), named("p2", "Mochi", "cat", 3), named("p3", "Shadow", "dog", 2, true)]);
      await setupApp(page, { handlers: [fake.handler] });
      await page.goto("/");
      const row = page.getByRole("region", { name: "Pets", exact: true });
      await row.getByRole("link", { name: "Biscuit, 6 photos" }).waitFor();
      await expect(row.getByRole("link", { name: "Mochi, 3 photos" })).toBeVisible();
      await expect(row.getByRole("link", { name: /Shadow/ })).toHaveCount(0);
      await expect(row.getByRole("link", { name: /^(Dogs|Cats), / })).toHaveCount(0);
      await expect(row.getByRole("link", { name: "See all" })).toHaveAttribute("href", "/pets");
      await check("with-named-pets");
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("Collections has no Pets row while the owner has named none", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("petkind-collections", width, scheme), problems);
      const fake = backend([]);
      await setupApp(page, { handlers: [fake.handler] });
      const named = page.waitForResponse((r) => r.url().endsWith("/assets/pets/named"));
      await page.goto("/");
      await page.getByRole("heading", { level: 1, name: "Collections" }).waitFor();
      await named;
      await page.getByRole("region", { name: "Recently added" }).waitFor();
      await expect(page.getByRole("region", { name: "Pets", exact: true })).toHaveCount(0);
      await expect(page.getByRole("link", { name: /^(Dogs|Cats), / })).toHaveCount(0);
      await check("without-named-pets");
      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("a cat that is a dog goes in a dog pet from the viewer and from a selection, and 'It's a dog' needs no pet", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("petkind-fix", width, scheme), problems);
      const fake = backend([named("p1", "Biscuit", "dog", 6)]);
      await setupApp(page, { handlers: [fake.handler] });

      // The viewer: the animal is called a cat; "Add to pet…" offers the dog pet, and the choice makes it a dog.
      await page.goto("/photos");
      const panel = await open(page, /Open IMG_00002/);
      await panel.getByText("not named").waitFor();
      await check("viewer-cat");
      await panel.getByRole("button", { name: "Add to pet…" }).click();
      const add = page.getByRole("dialog", { name: "Add to pet" });
      await add.waitFor();
      await expect(add.getByRole("radio", { name: "Cat" })).toBeChecked();
      await add.getByLabel("Pet", { exact: true }).selectOption({ label: "Biscuit (dog)" });
      await check("viewer-add-sheet");
      await add.getByRole("button", { name: "Add photos" }).click();
      await page.getByText("Added 1 photo to Biscuit").waitFor();
      expect(fake.last()).toEqual({ method: "POST", path: "/assets/pets/named/p1/photos", body: { detection_ids: ["x1"] } });
      await panel.getByRole("link", { name: "Biscuit" }).waitFor();
      await check("viewer-added");

      // "It's a dog" needs no pet: from a cat (the server forgot the pet, as it does when the kind changes under a pet).
      fake.calls.length = 0;
      await panel.getByRole("button", { name: "More for Dog" }).click();
      await page.getByRole("menuitem", { name: "It’s a cat" }).waitFor();
      await check("viewer-kind-menu");
      await page.getByRole("menuitem", { name: "It’s a cat" }).click();
      await page.getByText(/^Marked as a cat/).waitFor();
      expect(fake.last()).toEqual({ method: "POST", path: "/assets/pets/detections/x1/kind", body: { kind: "cat" } });
      await panel.getByText("not named").waitFor();
      await panel.getByRole("button", { name: "More for Cat" }).click();
      await page.getByRole("menuitem", { name: "It’s a dog" }).click();
      await page.getByText("Marked as a dog").waitFor();
      expect(fake.last()).toEqual({ method: "POST", path: "/assets/pets/detections/x1/kind", body: { kind: "dog" } });
      await expect(panel.getByText(/Dog/).first()).toBeVisible();
      await check("viewer-its-a-dog");
      // Escape closes the Info panel first (a sheet on a phone), then the viewer.
      for (let i = 0; i < 3 && (await page.getByRole("dialog", { name: /IMG_00002/ }).count()) > 0; i++) {
        await page.keyboard.press("Escape");
        await page.waitForTimeout(400);
      }
      await expect(page.getByRole("dialog", { name: /IMG_00002/ })).toHaveCount(0);

      // From a selection: the photo has no dog, so the owner is asked, the animal marked as a cat and chosen already.
      fake.calls.length = 0;
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00002/ }).click();
      const bar = page.getByRole("toolbar", { name: "Selection" });
      const direct = bar.getByRole("button", { name: "Name pet…" });
      if (await direct.count()) await direct.click();
      else {
        await bar.getByRole("button", { name: "More actions" }).click();
        await page.getByRole("menuitem", { name: "Name pet…" }).click();
      }
      const sheet = page.getByRole("dialog", { name: "Name pet" });
      await sheet.waitFor();
      await sheet.getByLabel("Pet", { exact: true }).selectOption({ label: "Biscuit (dog)" });
      await sheet.getByRole("button", { name: "Add photos" }).click();
      const which = page.getByRole("dialog", { name: "Which one is Biscuit?" });
      await which.waitFor();
      await which.getByRole("button", { name: "Animal 1 of 1, cat" }).waitFor();
      await expect(which.getByRole("button", { name: "Animal 1 of 1, cat" })).toHaveAttribute("aria-pressed", "true");
      await expect(which.getByText(/Biscuit is a dog/)).toBeVisible();
      await check("selection-which");
      await which.getByRole("button", { name: "Add" }).click();
      await page.getByText("Added 1 photo to Biscuit").waitFor();
      expect(fake.calls.map((c) => [c.path, c.body])).toEqual([
        ["/assets/pets/named/p1/photos", { asset_ids: ["a2"] }],
        ["/assets/pets/named/p1/photos", { detection_ids: ["x1"] }],
      ]);
      await check("selection-done");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
