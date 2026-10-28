// Named pets at phone and desktop widths: the Pets page's "Your pets" row, naming a pet from a selection (with the photo that has two
// animals asking which one, and the photos that could not join explained), a pet's page with "Is this X?" reviewed in bulk and one by one,
// rename, hide, cover, merge, delete and "Not this pet", the viewer's Pets row, and the pet in search (the chip made of the words, the
// filter, the filter sheet) and in a smart album's rule. Set SHOT_DIR to save the screenshots.
import { expect, test, type Page } from "@playwright/test";
import { pageOf, timelineOf, type FakeAsset } from "./fakeLibrary";
import { json, keepViewerBarsShown, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG = "Biscuit the second of the name_with_an_unbroken_run_of_characters_that_could_overflow";

interface FakePet {
  id: string;
  name: string;
  kind: "dog" | "cat";
  hidden: boolean;
  photos: number;
}
interface Call {
  method: string;
  path: string;
  body: unknown;
}

const box = [0.35, 0.3, 0.3, 0.3];
const SUMMARY = { any: 9, kinds: [{ kind: "dog", photos: 6, cover: { asset_id: "a0", box, preview_url: "/fixture-thumbs/0.svg" } }, { kind: "cat", photos: 3, cover: { asset_id: "a6", box, preview_url: "/fixture-thumbs/6.svg" } }], scanned: 30, total: 30 };

/** A stand-in for the named pets' endpoints, with the pets kept in memory so the page sees what it changed. */
function backend(start: FakePet[], opts: { likely?: number; maybe?: number } = {}) {
  const pets = start.map((p) => ({ ...p }));
  const calls: Call[] = [];
  let mine: FakeAsset[] = [];
  const proposals: Record<string, { likely: string[]; maybe: string[] }> = {};
  for (const p of pets) proposals[p.id] = { likely: Array.from({ length: opts.likely ?? 0 }, (_, i) => `${p.id}-l${i}`), maybe: Array.from({ length: opts.maybe ?? 0 }, (_, i) => `${p.id}-m${i}`) };
  let seq = 10;
  const find = (id: string) => pets.find((p) => p.id === id);
  const view = (p: FakePet) => {
    const left = proposals[p.id] ? proposals[p.id]!.likely.length + proposals[p.id]!.maybe.length : 0;
    return {
      id: p.id,
      name: p.name,
      kind: p.kind,
      hidden: p.hidden,
      photos: p.photos,
      detections: p.photos,
      pending: p.hidden ? 0 : left,
      // Waffle's cover fills most of its photo, as a pet often does: the round crop must still be filled, not letterboxed.
      cover: p.photos > 0 ? { detection_id: `d-${p.id}`, asset_id: "a0", box: p.id === "p2" ? [0.05, 0.1, 0.9, 0.85] : box, preview_url: "/fixture-thumbs/0.svg" } : null,
    };
  };
  const sug = (id: string, tier: "likely" | "maybe") => {
    const n = Number(/(\d+)$/.exec(id)![1]) + (tier === "maybe" ? 10 : 0) + 3;
    return { detection_id: id, asset_id: `a${n}`, box: [0.3, 0.3, 0.4, 0.4], preview_url: `/fixture-thumbs/${n}.svg`, kind: "dog", similarity: tier === "likely" ? 0.8 : 0.6, tier };
  };
  const record = (route: Parameters<Handler>[0], method: string, path: string) => {
    const body = method === "GET" ? null : route.request().postDataJSON();
    calls.push({ method, path, body });
    return body as Record<string, unknown> | null;
  };
  const handler: Handler = (route, url, method) => {
    const p = url.pathname;
    const ok = (body: unknown, status = 200) => {
      void route.fulfill(json(body, status));
      return true;
    };
    if (p === "/assets/pets/status") return ok({ enabled: true });
    if (p === "/assets/pets") return ok(SUMMARY);
    if (p === "/assets/pets/timeline") return ok(timelineOf(mine.slice(0, 9), false));
    if (p === "/assets/pets/photos") {
      const page = pageOf(mine.slice(0, 9), url.searchParams);
      return ok({ ...page, items: page.items.map((a) => ({ ...a, rating: 0, archived: false, hidden: false, edited: false })) });
    }
    if (p === "/assets/pets/named" && method === "GET") return ok({ items: pets.map(view) });
    if (p === "/assets/pets/named" && method === "POST") {
      const b = record(route, method, p) as { name: string; kind: "dog" | "cat"; asset_ids: string[] };
      const pet: FakePet = { id: `p${seq++}`, name: b.name, kind: b.kind, hidden: false, photos: 0 };
      pets.push(pet);
      proposals[pet.id] = { likely: Array.from({ length: opts.likely ?? 0 }, (_, i) => `${pet.id}-l${i}`), maybe: Array.from({ length: opts.maybe ?? 0 }, (_, i) => `${pet.id}-m${i}`) };
      // The photo a1 has two animals, a2 has none, the others one each.
      const ambiguous = b.asset_ids.includes("a1")
        ? [{ asset_id: "a1", preview_url: "/fixture-thumbs/1.svg", candidates: [{ detection_id: "d1a", kind: "dog", box: [0.08, 0.2, 0.3, 0.4] }, { detection_id: "d1b", kind: "dog", box: [0.55, 0.45, 0.3, 0.35] }] }]
        : [];
      const skipped = b.asset_ids.includes("a2") ? [{ asset_id: "a2", reason: "no_animal" }] : [];
      pet.photos = b.asset_ids.filter((a) => a !== "a1" && a !== "a2").length;
      return ok({ pet: view(pet), assigned: pet.photos, ambiguous, skipped }, 200);
    }
    const one = /^\/assets\/pets\/named\/(p\d+)(\/.*)?$/.exec(p);
    if (one) {
      const pet = find(one[1]!);
      if (!pet) return ok({ error: "not found" }, 404);
      const sub = one[2] ?? "";
      if (sub === "" && method === "GET") return ok(view(pet));
      if (sub === "" && method === "PATCH") {
        const b = record(route, method, p) as { name?: string; hidden?: boolean };
        if (b.name !== undefined) pet.name = b.name;
        if (b.hidden !== undefined) pet.hidden = b.hidden;
        return ok(view(pet));
      }
      if (sub === "" && method === "DELETE") {
        record(route, method, p);
        pets.splice(pets.indexOf(pet), 1);
        return ok({}, 204);
      }
      if (sub === "/merge") {
        const b = record(route, method, p) as { into: string };
        const into = find(b.into)!;
        into.photos += pet.photos;
        pets.splice(pets.indexOf(pet), 1);
        return ok({ into: into.id, detections: pet.photos });
      }
      if (sub === "/detections") {
        return ok({ items: [1, 2, 3, 4, 5, 6].map((n) => ({ detection_id: `c${n}`, asset_id: `a${n}`, box, preview_url: `/fixture-thumbs/${n}.svg` })), next_cursor: null });
      }
      if (sub === "/timeline") return ok(timelineOf(mine.slice(0, Math.max(pet.photos, 1)), false));
      if (sub === "/photos" && method === "GET") {
        const page = pageOf(mine.slice(0, Math.max(pet.photos, 1)), url.searchParams);
        return ok({ ...page, items: page.items.map((a) => ({ ...a, rating: 0, archived: false, hidden: false, edited: false })) });
      }
      if (sub === "/photos" && method === "POST") {
        const b = record(route, method, p) as { detection_ids?: string[] };
        pet.photos += b.detection_ids?.length ?? 0;
        return ok({ pet: view(pet), assigned: b.detection_ids?.length ?? 0, ambiguous: [], skipped: [] });
      }
      if (sub === "/photos/remove") {
        const b = record(route, method, p) as { asset_ids?: string[] };
        pet.photos = Math.max(0, pet.photos - (b.asset_ids?.length ?? 0));
        return ok({ removed: b.asset_ids?.length ?? 0 });
      }
      if (sub === "/suggestions") {
        const q = proposals[pet.id] ?? { likely: [], maybe: [] };
        const tier = url.searchParams.get("tier");
        const items = [...(tier !== "maybe" ? q.likely.map((id) => sug(id, "likely")) : []), ...(tier !== "likely" ? q.maybe.map((id) => sug(id, "maybe")) : [])];
        return ok({ items, counts: { likely: q.likely.length, maybe: q.maybe.length } });
      }
      if (sub === "/suggestions/answer") {
        const b = record(route, method, p) as { confirm?: string[]; reject?: string[] };
        const q = proposals[pet.id]!;
        const gone = new Set([...(b.confirm ?? []), ...(b.reject ?? [])]);
        q.likely = q.likely.filter((id) => !gone.has(id));
        q.maybe = q.maybe.filter((id) => !gone.has(id));
        pet.photos += b.confirm?.length ?? 0;
        return ok({ confirmed: b.confirm?.length ?? 0, rejected: b.reject?.length ?? 0, skipped: 0 });
      }
    }
    const det = /^\/assets\/pets\/detections\/([^/]+)\/(assign|reject)$/.exec(p);
    if (det) {
      record(route, method, p);
      if (det[2] === "reject") return ok({}, 204);
      return ok({ pet: null });
    }
    // The viewer: a2 has a named pet and an animal nobody named, a3 a single named one.
    const animals = /^\/assets\/(a\d+)\/pets$/.exec(p);
    if (animals) {
      const items =
        animals[1] === "a2"
          ? [
              { id: "x1", kind: "dog", box: [0.1, 0.1, 0.3, 0.3], score: 0.9, pet: { id: pets[0]?.id ?? "p1", name: pets[0]?.name ?? "Biscuit" } },
              { id: "x2", kind: "dog", box: [0.55, 0.5, 0.3, 0.3], score: 0.8, pet: null },
            ]
          : [];
      return ok({ items, scanned: true });
    }
    if (p === "/assets/search/facets") {
      return ok({ total: 30, favorites: 0, with_location: 0, types: [], cameras: [], lenses: [], places: [], years: [], pets: [{ value: "any", count: 9 }, { value: "dog", count: 6 }, { value: "cat", count: 3 }] });
    }
    if (method === "POST" && p === "/assets/albums") {
      record(route, method, p);
      return ok({ id: "al1", title: "Holiday", description: null, photo_count: 0, cover: null, first_at: null, last_at: null, created_at: "", updated_at: "" }, 201);
    }
    if (method === "POST" && p === "/assets/albums/al1/assets") {
      record(route, method, p);
      return ok({ count: 4 });
    }
    if (p === "/assets/albums/preview") return ok({ count: 12, capped: false, items: [], rules: { match: "all", archived: "include", rules: [] } });
    if (p === "/assets/tags" && method === "GET") return ok({ items: [] });
    return false;
  };
  return {
    handler,
    calls,
    pets,
    start(assets: FakeAsset[]) {
      mine = assets;
    },
    last: () => calls.at(-1),
  };
}

const rowOf = (page: Page) => page.getByRole("region", { name: "Your pets" });

for (const { width, scheme } of RUNS) {
  test.describe(`named pets ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("naming a pet from a selection, the photo with two animals, and reviewing the proposals", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("pets4-name", width, scheme), problems);
      const fake = backend([], { likely: 4, maybe: 3 });
      const assets = await setupApp(page, { handlers: [fake.handler] });
      fake.start(assets.slice(0, 9));

      // The Pets page with no pet named yet: the row says how.
      await page.goto("/pets");
      await rowOf(page).getByText(/Pick a few photos of your dog or cat/).waitFor();
      await page.getByRole("button", { name: /Open IMG_00000/ }).waitFor();
      await check("pets-no-pet-yet");

      // Name a pet: selection mode asks for the photos, then the sheet asks for a name and a kind.
      await rowOf(page).getByRole("button", { name: "Name a pet" }).click();
      await page.getByText("Select photos of your pet").waitFor();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await page.getByRole("button", { name: /Select IMG_00002/ }).click();
      await check("name-select");
      const bar = page.getByRole("toolbar", { name: "Selection" });
      await bar.getByRole("button", { name: "Name pet…" }).click();
      const sheet = page.getByRole("dialog", { name: "Name pet" });
      await sheet.waitFor();
      await sheet.getByLabel("Name", { exact: true }).fill(LONG);
      await check("name-sheet");
      await sheet.getByRole("button", { name: "Save" }).click();

      // One photo has two dogs: it is shown with each outlined, and the right one is tapped.
      const which = page.getByRole("dialog", { name: `Which one is ${LONG}?` });
      await which.waitFor();
      await which.getByRole("button", { name: "Animal 1 of 2, dog" }).waitFor();
      await expect(which.getByRole("button", { name: "Add" })).toBeDisabled();
      await check("ambiguous");
      await which.getByRole("button", { name: "Animal 2 of 2, dog" }).click();
      await expect(which.getByRole("button", { name: "Animal 2 of 2, dog" })).toHaveAttribute("aria-pressed", "true");
      await check("ambiguous-chosen");
      await which.getByRole("button", { name: "Add" }).click();

      // One photo had no animal: said in plain words, and the pet was made with the others.
      const done = page.getByRole("dialog", { name: `Made ${LONG}` });
      await done.waitFor();
      await done.getByText("1 photo has no dog or cat that was found").waitFor();
      await check("name-skipped");
      await done.getByRole("button", { name: "Done" }).click();
      await page.getByText(/^Made .* with 2 photos$/).waitFor();
      expect(fake.calls.map((c) => [c.method, c.path, c.body])).toEqual([
        ["POST", "/assets/pets/named", { name: LONG, kind: "dog", asset_ids: ["a0", "a1", "a2"] }],
        ["POST", "/assets/pets/named/p10/photos", { detection_ids: ["d1b"] }],
      ]);
      await page.getByRole("toolbar", { name: "Selection" }).waitFor({ state: "detached" });
      const card = rowOf(page).getByRole("link", { name: new RegExp(`^${LONG.slice(0, 20)}`) });
      await card.waitFor();
      await check("name-done");

      // The pet's page, with "Is this X?" waiting: the likely animals first, then the maybe ones.
      await page.getByRole("button", { name: "Open", exact: true }).click();
      await page.getByRole("heading", { level: 1, name: LONG }).waitFor();
      const section = page.getByRole("region", { name: "Suggestions" });
      await section.getByRole("heading", { name: `Is this ${LONG}?` }).waitFor();
      await expect(section.getByRole("button", { name: "See the whole photo" })).toHaveCount(4);
      await expect(section.getByRole("radio", { name: "Likely (4)" })).toBeChecked();
      await page.getByText("7 photos to confirm").first().waitFor();
      await check("pet-page");

      // One by one: No on the first, Yes on the second.
      await section.getByRole("button", { name: `No, this is not ${LONG}` }).first().click();
      await expect(section.getByRole("button", { name: "See the whole photo" })).toHaveCount(3);
      expect((fake.last()!.body as { reject: string[] }).reject).toEqual(["p10-l0"]);
      await section.getByRole("button", { name: `Yes, this is ${LONG}` }).first().click();
      await expect(section.getByRole("button", { name: "See the whole photo" })).toHaveCount(2);
      expect((fake.last()!.body as { confirm: string[] }).confirm).toEqual(["p10-l1"]);
      await check("review-one-by-one");

      // The whole photo with the animal outlined.
      await section.getByRole("button", { name: "See the whole photo" }).first().click();
      const whole = page.getByRole("dialog", { name: `Is this ${LONG}?` });
      await whole.waitFor();
      await expect(whole.getByRole("button", { name: "Not a face" })).toHaveCount(0);
      await check("review-whole-photo");
      await page.keyboard.press("Escape");
      await whole.waitFor({ state: "detached" });

      // In bulk: the rest of the likely ones at once (after asking), then the maybe tier.
      await section.getByRole("button", { name: "Yes to these 2" }).click();
      const sure = page.getByRole("dialog", { name: `Add 2 to ${LONG}?` });
      await sure.waitFor();
      await check("review-bulk-yes");
      await sure.getByRole("button", { name: "Yes to 2" }).click();
      await page.getByText(/^Added 2 photos to /).waitFor();
      expect((fake.last()!.body as { confirm: string[] }).confirm).toEqual(["p10-l2", "p10-l3"]);
      // Nothing likely is left: the maybe ones show.
      await expect(section.getByRole("button", { name: "See the whole photo" })).toHaveCount(3);
      await check("review-maybe");
      await section.getByRole("button", { name: "No to these 3" }).click();
      const none = page.getByRole("dialog", { name: /^None of these 3 are/ });
      await none.waitFor();
      await check("review-bulk-no");
      await none.getByRole("button", { name: "No to 3" }).click();
      await section.waitFor({ state: "detached" });
      expect((fake.last()!.body as { reject: string[] }).reject).toEqual(["p10-m0", "p10-m1", "p10-m2"]);
      await check("review-done");

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("rename, hide, cover, merge, delete, 'Not this pet', and the pet in the viewer", async ({ page }) => {
      test.setTimeout(150_000);
      await keepViewerBarsShown(page);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("pets4-manage", width, scheme), problems);
      const fake = backend([
        { id: "p1", name: "Biscuit", kind: "dog", hidden: false, photos: 6 },
        { id: "p2", name: "Waffle", kind: "dog", hidden: false, photos: 3 },
        { id: "p3", name: "Mochi", kind: "cat", hidden: true, photos: 2 },
      ]);
      const assets = await setupApp(page, { handlers: [fake.handler] });
      fake.start(assets.slice(0, 9));

      // The row: the named pets, the hidden one last and dimmed.
      await page.goto("/pets");
      const row = rowOf(page);
      await row.getByRole("link", { name: "Biscuit, 6 photos" }).waitFor();
      await expect(row.getByText("Hidden")).toHaveCount(1);
      await check("pets-row");
      // Every cover fills its circle: the picture reaches past each edge of the crop (no empty band), the big one included.
      await expect(row.locator('.face-crop[data-ready="true"]')).toHaveCount(3);
      const gaps = await row.locator(".face-crop").evaluateAll((crops) =>
        crops.map((c) => {
          const o = c.getBoundingClientRect();
          const i = c.querySelector("img")!.getBoundingClientRect();
          return Math.max(i.left - o.left, i.top - o.top, o.right - i.right, o.bottom - i.bottom);
        }),
      );
      for (const g of gaps) expect(g).toBeLessThanOrEqual(0.5);
      await row.getByRole("link", { name: "Biscuit, 6 photos" }).click();
      await page.getByRole("heading", { level: 1, name: "Biscuit" }).waitFor();
      await page.getByRole("button", { name: /Open IMG_00000/ }).waitFor();
      await check("pet-page");

      // Rename (a pet is never without a name).
      await page.getByRole("button", { name: "More" }).click();
      await check("pet-menu");
      await page.getByRole("menuitem", { name: "Rename" }).click();
      const name = page.getByLabel("Name", { exact: true });
      await name.fill("");
      await page.getByRole("button", { name: "Save", exact: true }).click();
      await page.getByText("Give your pet a name.").waitFor();
      await check("pet-rename-blank");
      await name.fill("Biscuit Two");
      await page.getByRole("button", { name: "Save", exact: true }).click();
      await page.getByRole("heading", { level: 1, name: "Biscuit Two" }).waitFor();
      expect(fake.last()).toEqual({ method: "PATCH", path: "/assets/pets/named/p1", body: { name: "Biscuit Two" } });

      // Cover: the pet's animals as crops.
      await page.getByRole("button", { name: "Change cover" }).click();
      const cover = page.getByRole("dialog", { name: "Change cover of Biscuit Two" });
      await cover.getByRole("list", { name: "Animals" }).waitFor();
      await check("pet-cover");
      await page.keyboard.press("Escape");

      // Selecting photos and "Not this pet".
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00000/ }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("pet-select");
      await page.getByRole("button", { name: "Not this pet" }).click();
      const notThis = page.getByRole("dialog", { name: "Not Biscuit Two?" });
      await notThis.waitFor();
      await check("pet-not-this-pet");
      await notThis.getByRole("button", { name: "Not this pet" }).click();
      await page.getByText("Took 2 photos out of Biscuit Two").waitFor();
      expect(fake.last()).toEqual({ method: "POST", path: "/assets/pets/named/p1/photos/remove", body: { asset_ids: ["a0", "a1"] } });

      // The whole pet saved as an album goes by the {pet} scope.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: "Select all" }).click();
      await page.getByRole("toolbar", { name: "Selection" }).getByRole("button", { name: "Save as album" }).click();
      const save = page.getByRole("dialog", { name: /^Save \d+ photos as an album$/ });
      await save.waitFor();
      await save.getByLabel("Album title").fill("Holiday");
      await save.getByRole("button", { name: "Create album" }).click();
      await page.getByText(/^Created Holiday with/).waitFor();
      expect(fake.last()).toEqual({ method: "POST", path: "/assets/albums/al1/assets", body: { scope: { pet: "p1" }, except: [] } });
      await page.getByRole("button", { name: "Done" }).click();

      // Merge: only the other dog is offered.
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Merge into…" }).click();
      const merge = page.getByRole("dialog", { name: "Merge Biscuit Two into…" });
      const choices = merge.getByRole("group", { name: "Merge into" });
      await choices.getByRole("radio", { name: /Waffle/ }).waitFor();
      await expect(choices.getByRole("radio")).toHaveCount(1);
      await check("pet-merge");
      await page.keyboard.press("Escape");

      // Hide, then delete after asking.
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Hide" }).click();
      await page.getByText(/^Hidden: no suggestions/).waitFor();
      expect(fake.last()).toEqual({ method: "PATCH", path: "/assets/pets/named/p1", body: { hidden: true } });
      await check("pet-hidden");
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Delete pet" }).click();
      const del = page.getByRole("dialog", { name: "Delete Biscuit Two?" });
      await del.waitFor();
      await check("pet-delete");
      await del.getByRole("button", { name: "Delete pet" }).click();
      await page.getByRole("heading", { level: 1, name: "Pets" }).waitFor();
      expect(fake.last()).toEqual({ method: "DELETE", path: "/assets/pets/named/p1", body: null });
      await expect(rowOf(page).getByRole("link", { name: /^Biscuit/ })).toHaveCount(0);

      // Merging the other way: Waffle into the pet that is left of its kind, there is none, so it says so.
      await rowOf(page).getByRole("link", { name: "Waffle, 3 photos" }).click();
      await page.getByRole("heading", { level: 1, name: "Waffle" }).waitFor();
      await page.getByRole("button", { name: "More" }).click();
      await page.getByRole("menuitem", { name: "Merge into…" }).click();
      await page.getByText(/You have no other dog to merge into/).waitFor();
      await check("pet-merge-none");
      await page.keyboard.press("Escape");

      // The viewer: the pet an animal is in, "Not Name" and "Add to pet…" for the one nobody named.
      await page.goto("/photos");
      await page.getByRole("button", { name: /Open IMG_00002/ }).first().click();
      await page.getByRole("dialog", { name: "IMG_00002.jpg" }).waitFor();
      await page.keyboard.press("i");
      const panel = page.locator("#details-panel");
      await panel.getByRole("link", { name: "Waffle" }).waitFor();
      await panel.getByRole("button", { name: "Not Waffle, dog 1" }).waitFor();
      await check("viewer-pets");
      await panel.getByRole("button", { name: "Add to pet…, dog 2" }).click();
      const add = page.getByRole("dialog", { name: "Add to pet" });
      await add.waitFor();
      await check("viewer-add-to-pet");
      await add.getByLabel("Pet", { exact: true }).selectOption({ label: "Waffle (dog)" });
      await add.getByRole("button", { name: "Add photos" }).click();
      await page.getByText("Added 1 photo to Waffle").waitFor();
      expect(fake.last()).toEqual({ method: "POST", path: "/assets/pets/named/p2/photos", body: { detection_ids: ["x2"] } });
      await panel.getByRole("button", { name: "Not Waffle, dog 1" }).click();
      await page.getByText("Took 1 photo out of Waffle").waitFor();
      expect(fake.last()).toEqual({ method: "POST", path: "/assets/pets/named/p2/photos/remove", body: { detection_ids: ["x1"] } });
      await check("viewer-pets-after");

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("a pet in search (the words, the filter, the sheet) and in a smart album's rule", async ({ page }) => {
      test.setTimeout(150_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("pets4-search", width, scheme), problems);
      const fake = backend([
        { id: "p1", name: "Biscuit", kind: "dog", hidden: false, photos: 6 },
        { id: "p2", name: "Waffle", kind: "dog", hidden: false, photos: 3 },
      ]);
      const assets = await setupApp(page, { handlers: [fake.handler] });
      fake.start(assets.slice(0, 9));
      const searches: string[] = [];
      await page.route("**/assets/search?*", (route) => {
        const u = new URL(route.request().url());
        searches.push(u.search);
        void route.fulfill(
          json({
            items: assets.slice(0, 4).map((a) => ({ ...a, score: null, rating: 0, archived: false, hidden: false, edited: false })),
            next_cursor: null,
            ranked: false,
            total: 4,
            parsed: u.searchParams.get("q")
              ? { text: "at the beach", chips: [{ kind: "pet", label: "Biscuit", matched: "Biscuit", pet: [{ id: "p1", name: "Biscuit" }] }] }
              : null,
          }),
        );
      });

      // The words: the chip names the pet, and its x gives the words back with skip=pet.
      await page.goto("/search?q=Biscuit%20at%20the%20beach");
      const chips = page.getByRole("list", { name: "Understood from your search" });
      await chips.getByText("Biscuit").waitFor();
      await check("search-chip");
      await chips.getByRole("button", { name: "Remove pet Biscuit" }).click();
      await page.waitForURL(/skip=pet/);

      // The filter in the address: a chip with the pet's name, and "Save as album" from the results goes by the filter.
      await page.goto("/search?pet=" + "p1");
      // (An id that is not a UUID is not a filter: the page shows nothing filtered.)
      expect(searches.at(-1) ?? "").not.toContain("pet=");
      const id = "6f1c2b0e-0000-4000-8000-0000000000a1";
      fake.pets.push({ id, name: "Mochi", kind: "cat", hidden: false, photos: 2 });
      await page.goto(`/search?pet=${id}`);
      const active = page.getByRole("list", { name: "Active filters" });
      await active.getByText("Mochi").waitFor();
      expect(searches.at(-1)).toContain(`pet=${id}`);
      await check("search-filter-chip");

      // The filter sheet: pick pets by name.
      await page.getByRole("button", { name: /Filters/ }).click();
      const dialog = page.getByRole("dialog", { name: "Filters" });
      const group = dialog.getByRole("group", { name: "Named pets" });
      await group.getByRole("checkbox", { name: /Biscuit/ }).waitFor();
      await expect(group.getByRole("checkbox", { name: /Mochi/ })).toBeChecked();
      await group.scrollIntoViewIfNeeded();
      await check("filter-sheet");
      await group.getByRole("checkbox", { name: /Biscuit/ }).click();
      await dialog.getByRole("button", { name: "Show photos" }).click();
      await page.waitForURL(/pet=p1|pet=6f1c/);
      expect(decodeURIComponent(page.url())).toContain("pet=6f1c2b0e-0000-4000-8000-0000000000a1");

      // The smart album rule: "Pet is …".
      await page.goto("/albums");
      await page.getByRole("button", { name: "New", exact: true }).click();
      await page.getByRole("menuitem", { name: "New smart album" }).click();
      const editor = page.getByRole("dialog", { name: "New smart album" });
      await editor.waitFor();
      await editor.getByLabel("Kind of rule to add").selectOption("pet");
      await editor.getByRole("button", { name: "Add rule" }).click();
      const rule = editor.getByRole("group", { name: /Pets together/ });
      await rule.getByRole("checkbox", { name: /Waffle/ }).waitFor();
      await rule.getByRole("checkbox", { name: /Waffle/ }).click();
      await check("smart-rule");
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
