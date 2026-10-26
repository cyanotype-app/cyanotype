// The app's frame at phone and desktop widths: the tab bar (Collections, Photos, Search) on a phone, the sidebar on a
// desktop, the account and upload menus, and getting between the three places. Nothing may stick out sideways. Set
// SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { makeCheck, RUNS, setupApp, shotPrefix, viewportOf } from "./kit";

for (const { width, scheme } of RUNS) {
  test.describe(`shell ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("the tab bar or sidebar, and the menus", async ({ page }) => {
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("shell", width, scheme), problems);
      await setupApp(page, { count: 24, role: "admin" });
      const desktop = width >= 768;

      await page.goto("/");
      await expect(page.getByRole("heading", { level: 1, name: "Collections" })).toBeVisible();
      const tabs = page.locator(".app-tabbar");
      const sidebar = page.locator(".app-sidebar");
      if (desktop) {
        await expect(tabs).toBeHidden();
        await expect(sidebar).toBeVisible();
      } else {
        await expect(sidebar).toBeHidden();
        await expect(tabs.getByRole("link")).toHaveText(["Collections", "Photos", "Search"]);
        await expect(tabs.getByRole("link", { name: "Collections" })).toHaveAttribute("aria-current", "page");
      }
      await check("collections");

      const nav = desktop ? sidebar.getByRole("navigation", { name: "Main" }) : tabs;
      await nav.getByRole("link", { name: "Photos" }).click();
      await expect(page).toHaveURL(/\/photos$/);
      if (!desktop) await expect(tabs.getByRole("link", { name: "Photos" })).toHaveAttribute("aria-current", "page");
      await check("photos");

      await page.goto("/library");
      await expect(page).toHaveURL(/\/photos$/);

      await page.goto("/");
      const account = desktop ? sidebar.getByRole("button", { name: /someone/ }) : page.getByRole("button", { name: "Account" });
      await account.click();
      const menu = page.getByRole("menu");
      await expect(menu.getByRole("menuitem", { name: "Settings" })).toBeVisible();
      await expect(menu.getByRole("menuitem", { name: "Administration" })).toBeVisible();
      await check("account");
      await page.keyboard.press("Escape");
      await expect(menu).toBeHidden();

      await page.getByRole("button", { name: "Upload" }).first().click();
      await expect(page.getByRole("menuitem", { name: "Photos and videos" })).toBeVisible();
      await check("upload");
      await page.keyboard.press("Escape");

      await page.getByRole("link", { name: "Recently deleted" }).first().click();
      await expect(page).toHaveURL(/\/trash$/);
      // A library view keeps Collections lit on a phone.
      if (!desktop) await expect(tabs.getByRole("link", { name: "Collections" })).toHaveAttribute("aria-current", "page");
      await check("trash");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
