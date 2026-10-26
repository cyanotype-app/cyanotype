// Stacks in the timeline at phone and desktop widths: the badge on a stack's tile, the viewer's strip (step through the stack with
// the buttons and `[` `]`, set the cover, take a photo out, unstack), "Stack" in the selection bar and the Stacked / Every
// photo switch. Nothing may stick out sideways, and every button must be reachable. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, keepViewerBarsShown, makeCheck, RUNS, setupApp, shotPrefix, viewportOf } from "./kit";
import { pageOf, timelineOf, type FakeAsset } from "./fakeLibrary";

for (const { width, scheme } of RUNS) {
  test.describe(`stacks ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("a stack's tile, the viewer's strip and the controls", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("stacks", width, scheme), problems);
      await keepViewerBarsShown(page);
      const calls: string[] = [];
      const lists: string[] = [];
      const assets = await setupApp(page, { count: 12 });
      // Photos a2, a3 and a4 are a stack; the cover is a2. The fake library is newest first, so a4 is the earliest.
      const st = { members: ["a2", "a3", "a4"], cover: "a2", collapsed: true };
      const stacked = () => st.members.length > 1;
      const tiles = (): FakeAsset[] =>
        assets.filter((a) => !stacked() || !st.collapsed || !st.members.includes(a.id) || a.id === st.cover);
      const entry = (a: FakeAsset) =>
        stacked() && st.members.includes(a.id)
          ? { ...a, stack: st.collapsed ? { size: st.members.length, members: st.members } : { size: st.members.length } }
          : a;
      const byId = (id: string) => assets.find((a) => a.id === id)!;

      await page.route(
        (u) => /^\/assets(\/prefs|\/timeline|\/stacks(\/restore)?|\/a\d+\/stack(\/cover|\/remove)?)?$/.test(u.pathname),
        async (route) => {
          const req = route.request();
          const url = new URL(req.url());
          const p = url.pathname;
          const method = req.method();
          if (p === "/assets/prefs") {
            if (method === "PUT") st.collapsed = (req.postDataJSON() as { stacks_collapsed: boolean }).stacks_collapsed;
            return route.fulfill(json({ memories_enabled: false, library_sort: "taken", stacks_collapsed: st.collapsed }));
          }
          if (p === "/assets/timeline" && method === "GET") return route.fulfill(json(timelineOf(tiles(), false)));
          if (p === "/assets" && method === "GET") {
            lists.push(url.search);
            const mode = url.searchParams.get("stacks");
            if (mode) st.collapsed = mode === "collapsed";
            const page1 = pageOf(tiles(), url.searchParams);
            return route.fulfill(json({ ...page1, items: page1.items.map(entry) }));
          }
          if (p === "/assets/stacks" && method === "POST") {
            const body = req.postDataJSON() as { ids: string[] };
            calls.push(`stack ${body.ids.join(",")}`);
            const previous = body.ids.map((id) => ({ id, stack_id: null, stack_cover: false }));
            return route.fulfill(json({ size: body.ids.length, cover_id: body.ids[0], previous }));
          }
          if (p === "/assets/stacks/restore" && method === "POST") {
            const body = req.postDataJSON() as { items: { id: string }[] };
            calls.push(`restore ${body.items.map((i) => i.id).join(",")}`);
            return route.fulfill(json({ count: body.items.length }));
          }
          const m = /^\/assets\/(a\d+)\/stack(\/cover|\/remove)?$/.exec(p);
          if (!m) return route.fallback();
          const id = m[1]!;
          if (!st.members.includes(id) || !stacked()) return route.fulfill(json({ error: "not found" }, 404));
          if (method === "GET" && !m[2]) {
            const items = [...st.members].sort().reverse().map((x) => byId(x)); // capture order: oldest (highest index) first
            return route.fulfill(json({ cover_id: st.cover, manual: false, items }));
          }
          if (method === "POST" && m[2] === "/cover") {
            calls.push(`cover ${id}`);
            st.cover = id;
            return route.fulfill(json({ cover_id: id }));
          }
          if (method === "POST" && m[2] === "/remove") {
            calls.push(`remove ${id}`);
            st.members = st.members.filter((x) => x !== id);
            if (st.cover === id) st.cover = st.members[0] ?? "";
            return route.fulfill(json({ removed: id }));
          }
          if (method === "DELETE" && !m[2]) {
            calls.push(`unstack ${id}`);
            st.members = [];
            return route.fulfill(json({ count: 3 }));
          }
          return route.fallback();
        },
      );

      await page.goto("/photos");
      const tile = page.getByRole("button", { name: "Open IMG_00002.jpg, stack of 3 photos" });
      await expect(tile).toBeVisible();
      await expect(tile.getByText("3", { exact: true })).toBeVisible(); // the badge
      // The other two photos of the stack are not tiles.
      await expect(page.getByRole("button", { name: /Open IMG_00003.jpg/ })).toHaveCount(0);
      await page.getByRole("button", { name: "View options" }).click();
      await expect(page.getByRole("menuitemradio", { name: "Stacked" })).toHaveAttribute("aria-checked", "true");
      await page.keyboard.press("Escape");
      await check("timeline");

      // The viewer: the strip, stepping with the buttons and with the keys, then the cover.
      await tile.click();
      const dialog = page.getByRole("dialog");
      // It opens on the cover, which is the latest of the three.
      await expect(dialog.getByText("Photo 3 of 3 in this stack")).toBeVisible();
      await check("viewer");
      await dialog.getByRole("button", { name: "Previous photo in the stack" }).click();
      await expect(dialog.getByText("Photo 2 of 3 in this stack")).toBeVisible();
      // The strip marks the photo on show.
      await expect(dialog.getByRole("button", { name: "IMG_00003.jpg", exact: true })).toHaveAttribute("aria-current", "true");
      await page.keyboard.press("[");
      await expect(dialog.getByText("Photo 1 of 3 in this stack")).toBeVisible();
      await page.keyboard.press("]");
      await expect(dialog.getByText("Photo 2 of 3 in this stack")).toBeVisible();
      for (const name of ["Set as stack cover", "Remove from stack", "Unstack all"]) {
        const b = dialog.getByRole("button", { name });
        await b.scrollIntoViewIfNeeded();
        const box = (await b.boundingBox())!;
        expect(box.x, name).toBeGreaterThanOrEqual(0);
        expect(box.x + box.width, name).toBeLessThanOrEqual(width);
        expect(box.height, name).toBeGreaterThanOrEqual(32);
      }
      await dialog.getByRole("button", { name: "Set as stack cover" }).click();
      await expect(page.getByText("Cover changed")).toBeVisible();
      expect(calls).toContain("cover a3");
      await check("viewer-cover");
      // The timeline's own arrows still go to the next tile.
      await page.keyboard.press("ArrowRight");
      await expect(dialog.getByRole("region", { name: "Stack" })).toHaveCount(0);
      await page.keyboard.press("ArrowLeft");
      await expect(dialog.getByText("Photo 2 of 3 in this stack")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);

      // The new cover is the tile now.
      await expect(page.getByRole("button", { name: "Open IMG_00003.jpg, stack of 3 photos" })).toBeVisible();

      // Take one photo out: the viewer closes and the stack has two.
      await page.getByRole("button", { name: "Open IMG_00003.jpg, stack of 3 photos" }).click();
      await dialog.getByRole("button", { name: "Remove from stack" }).click();
      await expect(dialog).toHaveCount(0);
      expect(calls).toContain("remove a3");
      await expect(page.getByRole("button", { name: "Open IMG_00003.jpg" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Open IMG_00002.jpg, stack of 2 photos" })).toBeVisible();

      // Stack by hand from the selection bar.
      await page.getByRole("button", { name: "Select", exact: true }).click();
      const bar = page.getByRole("toolbar", { name: "Selection" });
      // "Stack" is on the bar on a wide screen, in its "More" menu on a phone.
      const openMore = async () => {
        if (!(await page.getByRole("menu", { name: "More actions" }).isVisible())) await bar.getByRole("button", { name: "More actions" }).click();
        return page.getByRole("menuitem", { name: "Stack", exact: true });
      };
      const stackItem = async () => ((await bar.getByRole("button", { name: "Stack", exact: true }).count()) ? bar.getByRole("button", { name: "Stack", exact: true }) : openMore());
      const disabled = async () => {
        const el = await stackItem();
        return (await el.getAttribute("aria-disabled")) === "true" || (await el.isDisabled());
      };
      expect(await disabled()).toBe(true);
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Select IMG_00000.jpg" }).click();
      await page.getByRole("button", { name: "Select IMG_00001.jpg" }).click();
      expect(await disabled()).toBe(false);
      await check("selection");
      await (await stackItem()).click();
      await expect(page.getByText("2 photos stacked")).toBeVisible();
      expect(calls).toContain("stack a0,a1");
      // Undo hands back what the server said the photos were.
      await page.getByRole("button", { name: "Undo" }).click();
      await expect.poll(() => calls.includes("restore a0,a1")).toBe(true);

      // "Every photo" lists them all; the switch is remembered by the server.
      await page.getByRole("button", { name: "View options" }).click();
      await page.getByRole("menuitemradio", { name: "Every photo" }).click();
      await expect.poll(() => lists.some((q) => q.includes("stacks=expanded"))).toBe(true);
      await expect(page.getByRole("button", { name: /Open IMG_00004.jpg/ })).toBeVisible();
      await check("every-photo");

      expect(problems).toEqual([]);
    });
  });
}
