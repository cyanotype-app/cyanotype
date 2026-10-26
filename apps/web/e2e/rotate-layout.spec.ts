// Rotate and crop at phone and desktop widths: the viewer's Crop button, the editor (turn, crop handles, straighten, ratios),
// the "updating" note after a save, and an edited photo's download menu and Info row. Nothing may stick out sideways, the
// handles must be touch targets, and a drag with a finger must move the crop. Set SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, LONG_NAME, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

for (const { width, scheme } of RUNS) {
  test.describe(`rotate and crop ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("edit a photo in the viewer, wait for the render, and use the edited photo's menus without overflow", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("a5", width, scheme), problems);
      const saved: Record<string, unknown>[] = [];
      let assets: Record<string, unknown>[] = [];
      // Photo a2 is edited by the test (the render takes two polls); a3 is edited from the start.
      let renderPolls = 0;
      let a2Saved = false;
      const recipeOf = (id: string) =>
        id === "a3" ? { rotate: 90, crop: { x: 0.1, y: 0.1, w: 0.6, h: 0.5 } } : id === "a2" && a2Saved ? { rotate: 90 } : null;

      const handlers: Handler[] = [
        (route, url, method) => {
          if (url.pathname === "/assets/edits" && method === "POST") {
            saved.push(route.request().postDataJSON() as Record<string, unknown>);
            a2Saved = true;
            renderPolls = 0;
            void route.fulfill(json({ updated: 1, unchanged: 0, missing: 0, failed: [] }));
            return true;
          }
          return false;
        },
      ];
      assets = (await setupApp(page, { handlers })) as unknown as Record<string, unknown>[];
      assets[3]!.edited = true;
      assets[3]!.width = 1800;
      assets[3]!.height = 1250;
      assets[2]!.original_name = LONG_NAME;

      // The fake library answers a photo's record and details itself; these two photos are answered here, ahead of it.
      await page.route(/\/assets\/a\d+(\/details)?$/, async (route) => {
        const url = new URL(route.request().url());
        const id = /^\/assets\/(a\d+)/.exec(url.pathname)![1]!;
        if (route.request().method() !== "GET" || (id !== "a2" && id !== "a3")) return route.fallback();
        const a = assets.find((x) => x.id === id)!;
        const recipe = recipeOf(id);
        if (url.pathname.endsWith("/details")) {
          return route.fulfill(
            json({
              id,
              date: { instant: a.sort_time, local: String(a.sort_time).slice(0, 19), offset: null, zone_known: false, weekday: "Wednesday", basis: "taken", timeline_day: String(a.sort_time).slice(0, 10) },
              location: null,
              camera: null,
              exposure: null,
              file: { name: a.original_name, format: "JPEG", raw: false, content_type: a.content_type, size_bytes: a.size_bytes, width: 4000, height: 3000, megapixels: "12 MP", color_space: null, software: null },
              files: [{ id, name: a.original_name, format: "JPEG", raw: false, size_bytes: a.size_bytes }],
              albums: [],
              burst: null,
              edits: { date_edited: false, location_edited: false, original_date: null, original_location: null, recipe, edited_copy_ready: false },
            }),
          );
        }
        const rendering = id === "a2" && a2Saved && ++renderPolls <= 2;
        return route.fulfill(
          json({
            ...a,
            albums: [],
            original_url: a.thumb_url,
            preview_url: a.thumb_url,
            recipe,
            rendering,
            edit_base_url: recipe && !rendering ? a.thumb_url : null,
            file_width: 4000,
            file_height: 3000,
          }),
        );
      });

      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_00000/ }).first().waitFor();
      await page.getByRole("button", { name: new RegExp(`Open ${LONG_NAME.slice(0, 20)}`) }).first().click();
      const viewer = page.getByRole("dialog", { name: LONG_NAME });
      await viewer.waitFor();
      const crop = viewer.getByRole("button", { name: "Rotate and crop", exact: true });
      await expect(crop).toBeEnabled();
      await check("viewer-crop-button");

      // The editor: turn with the key, show the crop handles, drag a corner, straighten, pick a shape.
      await crop.click();
      const editor = page.getByRole("dialog", { name: "Rotate and crop" });
      await editor.waitFor();
      await check("editor");
      await page.keyboard.press("r");
      await expect(editor.getByText(/Rotated 90° right/)).toBeVisible();
      await page.keyboard.press("c");
      const handles = editor.getByRole("button", { name: /^Crop (corner|edge)/ });
      await expect(handles).toHaveCount(8);
      // Every handle is a touch target, and the browser does not scroll or zoom while one is dragged.
      for (const h of await handles.all()) {
        const b = (await h.boundingBox())!;
        expect(b.width).toBeGreaterThanOrEqual(43.5);
        expect(b.height).toBeGreaterThanOrEqual(43.5);
        expect(await h.evaluate((el) => getComputedStyle(el).touchAction)).toBe("none");
      }
      await check("editor-crop");

      const box = editor.getByTestId("crop-box");
      const before = (await box.boundingBox())!;
      const se = editor.getByRole("button", { name: "Crop corner, bottom right" });
      const sb = (await se.boundingBox())!;
      const [cx, cy] = [sb.x + sb.width / 2, sb.y + sb.height / 2];
      if (width < 600) {
        // A finger: pointer events of type touch (what a phone sends), not a mouse.
        await se.dispatchEvent("pointerdown", { pointerId: 7, pointerType: "touch", isPrimary: true, button: 0, clientX: cx, clientY: cy, bubbles: true });
        await se.dispatchEvent("pointermove", { pointerId: 7, pointerType: "touch", isPrimary: true, clientX: cx - 40, clientY: cy - 30, bubbles: true });
        await se.dispatchEvent("pointerup", { pointerId: 7, pointerType: "touch", isPrimary: true, bubbles: true });
      } else {
        await page.mouse.move(cx, cy);
        await page.mouse.down();
        await page.mouse.move(cx - 60, cy - 40, { steps: 5 });
        await page.mouse.up();
      }
      const after = (await box.boundingBox())!;
      expect(after.width).toBeLessThan(before.width - 20);
      expect(after.height).toBeLessThan(before.height - 15);
      await check("editor-dragged");

      await editor.getByRole("slider").fill("3.5");
      await expect(editor.getByText(/straightened 3\.5° right/i)).toBeVisible();
      await editor.getByRole("button", { name: "16:9" }).click();
      await check("editor-straighten-ratio");

      // Save with Enter (a focused button would take the key as its own press): the recipe goes to the server and the viewer
      // says the picture is being updated.
      await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
      await page.keyboard.press("Enter");
      await expect(page.getByRole("dialog", { name: "Rotate and crop" })).toHaveCount(0);
      expect(saved).toHaveLength(1);
      const recipe = (saved[0] as { recipe: { rotate: number; straighten: number; crop: { w: number; h: number } } }).recipe;
      expect(recipe.rotate).toBe(90);
      expect(recipe.straighten).toBe(3.5);
      expect(recipe.crop.w).toBeGreaterThan(0.05);
      await expect(viewer.getByText("Updating the picture…")).toBeVisible();
      await check("viewer-updating");
      await expect(viewer.getByText("Updating the picture…")).toHaveCount(0, { timeout: 15_000 });
      await check("viewer-updated");
      await page.keyboard.press("Escape");

      // An edited photo: its caption says so, the download menu offers the original and the edited JPEG, the Info panel
      // describes the recipe, and the editor starts from the whole picture with a way back to the original.
      await page.getByRole("button", { name: /Open IMG_00003/ }).first().click();
      const edited = page.getByRole("dialog", { name: "IMG_00003.jpg" });
      await edited.waitFor();
      await expect(edited.getByText("Edited", { exact: true })).toBeVisible();
      await edited.getByRole("button", { name: "More" }).click();
      await expect(page.getByRole("menuitem", { name: "Download edited (JPEG)" })).toBeVisible();
      await expect(page.getByRole("menuitem", { name: /Download original/ })).toBeVisible();
      await check("edited-downloads");
      await page.keyboard.press("Escape");
      await page.keyboard.press("i");
      const panel = page.locator("#details-panel");
      await panel.waitFor();
      await expect(panel.getByText(/Rotated 90° right, cropped to 1800 × 2000/)).toBeVisible();
      await check("edited-info");
      await page.keyboard.press("Escape");
      await edited.getByRole("button", { name: "Rotate and crop", exact: true }).click();
      const again = page.getByRole("dialog", { name: "Rotate and crop" });
      await expect(again.getByRole("button", { name: "Revert to original" })).toBeVisible();
      await expect(again.getByRole("button", { name: "Save" })).toBeDisabled();
      await check("editor-edited");
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog", { name: "Rotate and crop" })).toHaveCount(0);
      await expect(edited).toBeVisible();

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
