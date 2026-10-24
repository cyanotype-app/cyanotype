// Closing the viewer never steps the browser history back out of the app, however quickly the closes come.
import { expect, test } from "@playwright/test";
import { setupApp } from "./kit";

const open = (page: import("@playwright/test").Page) => page.getByRole("button", { name: /Open IMG_00002/ }).first();
const viewer = (page: import("@playwright/test").Page) => page.getByRole("dialog", { name: "IMG_00002.jpg" });
const info = (page: import("@playwright/test").Page) => page.getByRole("complementary", { name: "Photo details" });

test.describe("viewer history", () => {
  test.beforeEach(async ({ page }) => {
    await setupApp(page, {});
    await page.goto("/library");
    await open(page).waitFor();
  });

  test("viewer, info panel, Escape twice stays in the app", async ({ page }) => {
    await open(page).click();
    await viewer(page).waitFor();
    await page.keyboard.press("i");
    await info(page).waitFor();
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await expect(info(page)).toHaveCount(0);
    await page.waitForTimeout(300);
    await expect(page).toHaveURL(/\/library/);
    // Nothing popped past the first entry of the app.
    expect(await page.evaluate(() => window.location.pathname)).toBe("/library");
  });

  test("a page opened straight onto the library closes the viewer without leaving", async ({ page }) => {
    const length = await page.evaluate(() => window.history.length);
    await open(page).click();
    await viewer(page).waitFor();
    await page.keyboard.press("Escape");
    await expect(viewer(page)).toHaveCount(0);
    await page.waitForTimeout(300);
    await expect(page).toHaveURL(/\/library/);
    expect(await page.evaluate(() => window.history.length)).toBe(length);
  });

  test("Back closes the info panel, then Escape closes the viewer", async ({ page }) => {
    await open(page).click();
    await viewer(page).waitFor();
    await page.keyboard.press("i");
    await info(page).waitFor();
    await page.goBack();
    await expect(info(page)).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(viewer(page)).toHaveCount(0);
    await expect(page).toHaveURL(/\/library/);
  });
});
