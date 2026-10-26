// The Google Takeout flow in a real browser with the tiny synthetic export in fixtures/takeout-synthetic: choose the folder,
// see the preview (counts, albums, options), import, and check what travels with each photo's check call. The server's answer
// to /assets/takeout/plan is the stored plan the real matcher makes of this folder (crates/cyanotype-takeout/tests/fixture.rs
// keeps it honest). Also a layout check (nothing scrolls sideways) at phone widths and in dark mode; SHOT_DIR saves screenshots.
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const SHOT_DIR = process.env.SHOT_DIR;
const FIXTURE = new URL("./fixtures/takeout-synthetic/Takeout", import.meta.url).pathname;
const PLAN = JSON.parse(readFileSync(new URL("./fixtures/takeout-plan.json", import.meta.url), "utf8")) as unknown;
const GP = join(FIXTURE, "Google Photos");
const text = (rel: string) => readFileSync(join(GP, rel), "utf8");

const RUNS: { width: number; scheme: "light" | "dark" }[] = [
  ...[320, 375, 1280].map((width) => ({ width, scheme: "light" as const })),
  { width: 360, scheme: "dark" },
];

type Check = { filename: string; takeout?: { sidecar?: string; album?: string; favorites: boolean } };

async function mock(page: Page, checks: Check[], planCalls: string[][]) {
  const json = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(
    (u) => /^\/(auth|assets)(\/|$)/.test(u.pathname),
    async (route) => {
      const p = new URL(route.request().url()).pathname;
      if (p === "/auth/me") return route.fulfill(json({ id: "u1", email: "me@example.org", role: "member" }));
      if (p === "/assets/takeout/plan") {
        planCalls.push((route.request().postDataJSON() as { paths: string[] }).paths);
        return route.fulfill(json(PLAN));
      }
      if (p === "/assets/check") {
        checks.push(route.request().postDataJSON() as Check);
        return route.fulfill(json({ status: "exists", asset_id: "a1" }));
      }
      if (p === "/assets/usage") return route.fulfill(json({}, 404));
      if (p.startsWith("/assets/")) return route.fulfill(json({ items: [], next_cursor: null, total: 0 }));
      return route.fulfill(json({}, 204));
    },
  );
}

function makeCheck(page: Page, prefix: string, problems: string[]) {
  return async (name: string) => {
    await page.waitForTimeout(150);
    const r = await page.evaluate(() => {
      const w = window.innerWidth;
      const bad: string[] = [];
      for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
        const b = el.getBoundingClientRect();
        if (b.width === 0) continue;
        if (b.right > w + 0.5 || b.left < -0.5) bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} [${Math.round(b.left)}..${Math.round(b.right)}]`);
      }
      return { scroll: document.documentElement.scrollWidth, w, bad: bad.slice(0, 6) };
    });
    if (r.scroll > r.w || r.bad.length) problems.push(`${name}: scrollWidth ${r.scroll} > ${r.w}; ${r.bad.join("; ")}`);
    if (SHOT_DIR) await page.screenshot({ path: join(SHOT_DIR, `${prefix}-${name}.png`), fullPage: true });
  };
}

for (const { width, scheme } of RUNS) {
  test.describe(`${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: { width, height: width === 1280 ? 800 : 740 }, hasTouch: width < 600 });

    test("choose the folder, preview, import with the metadata", async ({ page }) => {
      test.setTimeout(60_000);
      if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
      const problems: string[] = [];
      const check = makeCheck(page, `${width}${scheme === "dark" ? "-dark" : ""}-takeout`, problems);
      const checks: Check[] = [];
      const planCalls: string[][] = [];
      await mock(page, checks, planCalls);

      await page.goto("/settings/google-photos");
      await page.getByRole("heading", { name: "Google Photos" }).waitFor();
      await check("start");

      // Choosing the folder sends names only: no file bytes, and the mock saw every name once.
      await page.getByLabel("Takeout folder").setInputFiles(FIXTURE);
      const preview = page.getByRole("region", { name: "What will be imported" });
      await preview.waitFor();
      expect(planCalls).toHaveLength(1);
      expect(planCalls[0]).toHaveLength(13);
      expect(planCalls[0]!.every((p) => p.startsWith("Takeout/Google Photos/"))).toBe(true);
      await expect(preview.getByText("6 photos to import")).toBeVisible();
      await expect(preview.getByText(/5 photos with their Google Photos details/)).toBeVisible();
      await expect(preview.getByText(/1 photo without a details file/)).toBeVisible();
      await expect(preview.getByText(/1 album from named folders/)).toBeVisible();
      await expect(preview.getByText("Trip to Lisbon")).toBeVisible();
      await expect(preview.getByText(/Skipped: 1 video/)).toBeVisible();
      expect(checks).toHaveLength(0); // nothing is uploaded before the person confirms
      await check("preview");

      await page.getByRole("button", { name: "Import 6 photos" }).click();
      await page.getByRole("status", { name: "Import started" }).waitFor();
      await expect.poll(() => checks.length).toBe(6);
      await check("started");

      const by = Object.fromEntries(checks.map((c) => [c.filename, c]));
      expect(by["IMG_1234.JPG"]!.takeout).toEqual({
        sidecar: text("Photos from 2019/IMG_1234.JPG.supplemental-metadata.json"),
        favorites: true,
      });
      // The counter's sidecar is IMG_1234.JPG(1).json, the edited copy shares the original's.
      expect(JSON.parse(by["IMG_1234(1).JPG"]!.takeout!.sidecar!).description).toBe("The second one");
      expect(by["IMG_1234-edited.JPG"]!.takeout!.sidecar).toBe(by["IMG_1234.JPG"]!.takeout!.sidecar);
      // No sidecar, and not in an album: nothing to send.
      expect(by["no_sidecar.jpg"]!.takeout).toBeUndefined();
      // A named folder is an album; the truncated sidecar name was matched.
      expect(by["IMG_5678.jpg"]!.takeout!.album).toBe("Trip to Lisbon");
      const long = by["2019-05-12_a_very_long_file_name_that_goes_on_forever_and_ever.jpg"]!.takeout!;
      expect(long.album).toBe("Trip to Lisbon");
      expect(JSON.parse(long.sidecar!).description).toBe("Cut at 46 characters");
      // The year folder is not an album.
      expect(by["IMG_1234.JPG"]!.takeout!.album).toBeUndefined();
      // The video was never sent.
      expect(by["clip.mp4"]).toBeUndefined();

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("the options are honoured", async ({ page }) => {
      test.setTimeout(60_000);
      const checks: Check[] = [];
      await mock(page, checks, []);
      await page.goto("/settings/google-photos");
      await page.getByLabel("Takeout folder").setInputFiles(FIXTURE);
      await page.getByRole("region", { name: "What will be imported" }).waitFor();
      await page.getByLabel("Import favorites").uncheck();
      await page.getByLabel("Create albums from folders").uncheck();
      await page.getByRole("button", { name: "Import 6 photos" }).click();
      await expect.poll(() => checks.length).toBe(6);
      expect(checks.every((c) => c.takeout === undefined || (c.takeout.favorites === false && c.takeout.album === undefined))).toBe(true);
    });
  });
}
