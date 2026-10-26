// Settings: a grouped list on a phone (each section its own page with a way back), a section list beside the section on a
// desktop; every section at phone and desktop widths, light and dark. Nothing may stick out sideways. Set SHOT_DIR to save
// the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const GB = 1024 ** 3;
const usage = {
  originals_bytes: 38 * GB,
  trash_bytes: 2 * GB,
  stored_bytes: 40 * GB,
  derived_bytes: 3 * GB,
  derived_complete: true,
  photo_count: 18_200,
  raw_count: 640,
  file_count: 18_840,
  trash_count: 120,
  uploads: { count_24h: 12, bytes_24h: 600 * 1024 ** 2, count_30d: 900, bytes_30d: 12 * GB },
  limits: { quota_bytes: 50 * GB, daily_upload_bytes: 5 * GB, max_file_bytes: 2 * GB },
  daily: { used_bytes: GB, oldest_rolls_out_at: new Date(Date.now() + 5 * 3_600_000).toISOString() },
};
const sessions = [
  { id: "s1", created_at: "2026-10-01T10:00:00Z", expires_at: "2026-10-31T10:00:00Z", current: true },
  { id: "s2", created_at: "2026-10-05T10:00:00Z", expires_at: "2026-11-04T10:00:00Z", current: false },
];
let prefs = { memories_enabled: true, library_sort: "taken", stacks_collapsed: true };
const prefPuts: unknown[] = [];

const handlers: Handler[] = [
  (route, url, method) => {
    const p = url.pathname;
    if (p === "/assets/prefs") {
      if (method === "PUT") {
        const patch = route.request().postDataJSON() as Record<string, unknown>;
        prefPuts.push(patch);
        prefs = { ...prefs, ...patch } as typeof prefs;
      }
      void route.fulfill(json(prefs));
      return true;
    }
    if (p === "/assets/usage") {
      void route.fulfill(json(usage));
      return true;
    }
    if (p === "/auth/sessions") {
      void route.fulfill(json({ items: sessions }));
      return true;
    }
    if (p === "/auth/2fa") {
      void route.fulfill(json({ available: true, enabled: true, recovery_codes_remaining: 8, required: false }));
      return true;
    }
    if (p === "/auth/api-keys" && method === "GET") {
      void route.fulfill(
        json({
          items: [
            { id: "k1", name: "cyanotype CLI on studio-laptop", prefix: "cyk_abcdef", created_at: "2026-09-01T00:00:00Z", last_used_at: "2026-10-10T00:00:00Z", scope: "import", expires_at: null, expired: false },
          ],
        }),
      );
      return true;
    }
    return false;
  },
];

for (const { width, scheme } of RUNS) {
  test.describe(`settings ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("the section list, every section, and getting between them", async ({ page }) => {
      test.setTimeout(90_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("settings", width, scheme), problems);
      prefs = { memories_enabled: true, library_sort: "taken", stacks_collapsed: true };
      prefPuts.length = 0;
      await setupApp(page, { count: 12, handlers });
      // setupApp only answers /assets and /auth/me: the account's own pages ask /auth for the rest.
      await page.route(
        (u) => /^\/auth\/(sessions|2fa|api-keys)$/.test(u.pathname),
        (route) => {
          const url = new URL(route.request().url());
          return handlers[0]!(route, url, route.request().method());
        },
      );
      const desktop = width >= 768;

      await page.goto("/settings");
      if (desktop) {
        // The first section opens by itself, with the other sections listed beside it.
        await expect(page).toHaveURL(/\/settings\/display$/);
        const nav = page.getByRole("navigation", { name: "Settings" });
        await expect(nav.getByRole("link")).toHaveText(["Display", "Storage", "Google Photos", "Security", "Sessions", "API keys"]);
        await expect(page.getByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
      } else {
        await expect(page.getByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
        for (const name of ["Display", "Storage", "Google Photos", "Security", "Sessions", "API keys"]) {
          await expect(page.getByRole("main").getByRole("link", { name: new RegExp(`^${name}`) })).toBeVisible();
        }
      }
      await check("home");

      // Display: the switches write the preference.
      if (!desktop) await page.getByRole("main").getByRole("link", { name: /^Display/ }).click();
      await expect(page).toHaveURL(/\/settings\/display$/);
      await page.getByRole("switch", { name: /Show memories/ }).waitFor();
      await check("display");
      await page.getByRole("switch", { name: /Show memories/ }).click();
      await expect.poll(() => prefPuts.length).toBe(1);
      expect(prefPuts[0]).toEqual({ memories_enabled: false });
      await page.getByRole("radio", { name: "Added" }).click();
      await expect.poll(() => prefPuts.length).toBe(2);
      expect(prefPuts[1]).toEqual({ library_sort: "added" });

      for (const [path, ready, name] of [
        ["storage", /Your limits/, "storage"],
        ["sessions", "This browser", "sessions"],
        ["google-photos", "Choose the Takeout folder", "google-photos"],
        ["api-keys", "Install the command line", "api-keys"],
        ["security", /Two-factor authentication/, "security"],
      ] as const) {
        if (desktop) await page.getByRole("navigation", { name: "Settings" }).getByRole("link", { name: path === "api-keys" ? "API keys" : path === "google-photos" ? "Google Photos" : new RegExp(`^${path}$`, "i") }).click();
        else {
          await page.goto("/settings");
          await page.getByRole("main").getByRole("link", { name: new RegExp(`^${path === "api-keys" ? "API keys" : path === "google-photos" ? "Google Photos" : path}`, "i") }).click();
        }
        await expect(page).toHaveURL(new RegExp(`/settings/${path}$`));
        await (typeof ready === "string" ? page.getByText(ready) : page.getByText(ready)).first().waitFor();
        await check(name);
      }

      if (!desktop) {
        // The way back from a section is a chevron to the list.
        await page.getByRole("link", { name: /Settings/ }).first().click();
        await expect(page).toHaveURL(/\/settings$/);
      }

      await page.goto("/settings/nothing-here");
      await expect(page.getByRole("heading", { level: 1, name: "Page not found" })).toBeVisible();
      await check("not-found");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
