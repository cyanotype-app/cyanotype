// Shared pieces of the per-feature layout specs (`*-layout.spec.ts`): the widths and colour schemes every page is checked
// at, the overflow check (with optional screenshots), and a mocked library to run a feature's screens against.
import type { Page, Route } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { installLibrary, makeAssets, type FakeAsset } from "./fakeLibrary";

/** Set SHOT_DIR to save a screenshot of every checked state. */
export const SHOT_DIR = process.env.SHOT_DIR;

/** 320 to 414 px phones, a 1280 px desktop, and a dark 360 px phone. */
export const RUNS: { width: number; scheme: "light" | "dark" }[] = [
  ...[320, 360, 375, 390, 414, 1280].map((width) => ({ width, scheme: "light" as const })),
  { width: 360, scheme: "dark" },
];

export const viewportOf = (width: number) => ({ width, height: width === 1280 ? 800 : 740 });

export const LONG_NAME = "IMG_20240615_184233_a-really-quite-long-camera-generated-filename-with-no-spaces-at-all-HDR-PANO.jpg";

/** Assert that nothing sticks out of the viewport, and with SHOT_DIR save `<prefix>-<name>.png`. */
export function makeCheck(page: Page, prefix: string, problems: string[]) {
  if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
  return async function check(name: string) {
    await page.waitForTimeout(150);
    const r = await page.evaluate(() => {
      const w = window.innerWidth;
      const bad: string[] = [];
      for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
        const b = el.getBoundingClientRect();
        if (b.width === 0 || el.closest(".tile") || el.closest(".map-canvas") || el.closest(".memories-row") || el.closest(".face-crop") || el.classList.contains("slide-backdrop")) continue;
        if (b.right > w + 0.5 || b.left < -0.5) {
          bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} "${(el.textContent ?? "").trim().slice(0, 24)}" [${Math.round(b.left)}..${Math.round(b.right)}]`);
        }
      }
      if (document.documentElement.scrollWidth > w) {
        for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
          if (el.scrollWidth > w + 0.5 && !el.closest(".tile") && !el.closest(".map-canvas") && !el.closest(".memories-row") && !el.closest(".face-crop") && !el.classList.contains("slide-backdrop")) {
            bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} scrollWidth ${el.scrollWidth}`);
          }
        }
      }
      return { scroll: document.documentElement.scrollWidth, w, bad: bad.slice(0, 6) };
    });
    if (r.scroll > r.w || r.bad.length) problems.push(`${name}: scrollWidth ${r.scroll} > ${r.w}; offenders: ${r.bad.join("; ")}`);
    if (SHOT_DIR) await page.screenshot({ path: join(SHOT_DIR, `${prefix}-${name}.png`) });
  };
}

/** `a1-390`, `a1-360-dark`: the screenshots of one feature at one width. */
export const shotPrefix = (feature: string, width: number, scheme: "light" | "dark") => `${feature}-${width}${scheme === "dark" ? "-dark" : ""}`;

export const json = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });

/** One handler of a feature's mocked endpoints: return true when it answered. */
export type Handler = (route: Route, url: URL, method: string) => Promise<boolean> | boolean;

/**
 * A signed-in member with a library of `count` photos (the timeline, list, details and download endpoints of
 * `fakeLibrary`), the quiet defaults of the pages around it, and the feature's own `handlers` tried first. Whatever nobody
 * answers is a 404, so a screen that calls an endpoint the test forgot to mock fails visibly.
 */
export async function setupApp(page: Page, opts: { count?: number; handlers?: Handler[]; role?: "admin" | "member" } = {}) {
  const assets: FakeAsset[] = makeAssets(opts.count ?? 30, { yearsBack: 1 });
  await page.route("**/auth/me", (r) =>
    r.fulfill(json({ id: "u1", email: "someone.with.a.long.address@example-photo-library.org", role: opts.role ?? "member" })),
  );
  await installLibrary(page, assets, async (route) => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    for (const h of opts.handlers ?? []) if (await h(route, url, method)) return;
    const p = url.pathname;
    if (p === "/assets/prefs") return route.fulfill(json({ memories_enabled: false, library_sort: "taken" }));
    if (p === "/assets/memories") return route.fulfill(json({ enabled: false, day: "10-19", years: [] }));
    if (p === "/auth/hidden") return route.fulfill(json({ unlocked: false, expires_at: null }));
    if (p === "/assets/albums" && method === "GET") return route.fulfill(json({ items: [] }));
    if (method === "POST" || method === "PATCH" || method === "DELETE") return route.fulfill(json({ count: 1 }));
    return route.fulfill(json({ error: "not mocked" }, 404));
  });
  return assets;
}
