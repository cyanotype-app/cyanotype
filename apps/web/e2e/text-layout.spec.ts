// The viewer's Text panel in a real browser: the button (and `t`), the recognised text with Copy, the boxes drawn over the photo where
// the text is, QR codes listed as what they say (a web address as a link that is never opened for you, everything else as text only),
// the empty and not-read-yet states, and the search filter "Text in the photo"; at 320 to 1280 px and in dark mode, nothing may scroll
// sideways. Set SHOT_DIR to also save a screenshot of each state.
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { installLibrary, makeAssets, type FakeAsset } from "./fakeLibrary";

const SHOT_DIR = process.env.SHOT_DIR;
const RUNS: { width: number; scheme: "light" | "dark" }[] = [
  ...[320, 375, 414, 1024, 1280].map((width) => ({ width, scheme: "light" as const })),
  { width: 360, scheme: "dark" },
];

const LONG_LINE = "A very long line of recognised text with_an_unbroken_token_" + "x".repeat(90) + " and then some more words after it";
const box = (x: number, y: number, w: number, h: number) => [x, y, x + w, y, x + w, y + h, x, y + h];
const READ = {
  status: "read",
  text: ["HOFER KG", "Karl-Popper-Strasse 2", "Total 22,41", LONG_LINE].join("\n"),
  lines: [
    { t: "HOFER KG", s: 0.98, q: box(0.1, 0.08, 0.5, 0.06), w: [] },
    { t: "Karl-Popper-Strasse 2", s: 0.97, q: box(0.1, 0.18, 0.6, 0.05), w: [] },
    { t: "Total 22,41", s: 0.96, q: box(0.1, 0.5, 0.4, 0.06), w: [] },
    { t: LONG_LINE, s: 0.9, q: box(0.05, 0.7, 0.9, 0.05), w: [] },
  ],
  qr: [
    { t: "https://example.org/receipt?id=0123456789abcdef0123456789abcdef0123456789abcdef", q: box(0.6, 0.3, 0.25, 0.25), link: "https://example.org/receipt?id=0123456789abcdef0123456789abcdef0123456789abcdef" },
    { t: "javascript:alert(document.cookie)", q: box(0.1, 0.3, 0.2, 0.2) },
    { t: "WIFI:S:a-guest-network-with-a-rather-long-name;T:WPA;P:" + "p".repeat(60) + ";;", q: box(0.3, 0.3, 0.2, 0.2) },
  ],
};
const EMPTY = { status: "read", text: "", lines: [], qr: [] };
const PENDING = { status: "pending", text: "", lines: [], qr: [] };

async function mock(page: Page, assets: FakeAsset[], texts: Record<string, unknown>, seen: { opened: string[] }) {
  const json = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(
    (u) => /^\/auth(\/|$)/.test(u.pathname),
    (route) => route.fulfill(json({ id: "u1", email: "me@example.org", role: "member" })),
  );
  await installLibrary(page, assets, async (route) => {
    const p = new URL(route.request().url()).pathname;
    const m = /^\/assets\/(a\d+)\/text$/.exec(p);
    if (m) return route.fulfill(json(texts[m[1]!] ?? EMPTY));
    if (p === "/assets/search/facets")
      return route.fulfill(json({ total: 30, favorites: 0, with_location: 0, with_text: 12, types: [], cameras: [], lenses: [], places: [], years: [] }));
    if (p === "/assets/search")
      return route.fulfill(json({ items: assets.slice(0, 5), next_cursor: null, ranked: false, total: 5 }));
    if (p === "/assets/albums") return route.fulfill(json({ items: [] }));
    if (p === "/assets/usage") return route.fulfill(json({}, 404));
    return route.fulfill(json({}, 404));
  });
  // A click on a QR link must not leave the app in this test; record that it was never opened by itself.
  page.on("popup", (pp) => seen.opened.push(pp.url()));
}

function makeCheck(page: Page, prefix: string, problems: string[]) {
  return async (name: string) => {
    await page.waitForTimeout(200);
    const r = await page.evaluate(() => {
      const w = window.innerWidth;
      const bad: string[] = [];
      for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
        const b = el.getBoundingClientRect();
        if (b.width === 0 || el.closest(".text-body")) continue;
        if (b.right > w + 0.5 || b.left < -0.5) bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} [${Math.round(b.left)}..${Math.round(b.right)}]`);
      }
      return { scroll: document.documentElement.scrollWidth, w, bad: bad.slice(0, 6) };
    });
    if (r.scroll > r.w || r.bad.length) problems.push(`${name}: scrollWidth ${r.scroll} > ${r.w}; ${r.bad.join("; ")}`);
    if (SHOT_DIR) await page.screenshot({ path: join(SHOT_DIR, `${prefix}-${name}.png`) });
  };
}

for (const { width, scheme } of RUNS) {
  test.describe(`${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: { width, height: width >= 1024 ? 800 : 740 }, hasTouch: width < 600 });

    test("the Text panel: text, boxes, QR codes and the empty states fit", async ({ page }) => {
      test.setTimeout(60_000);
      if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
      const problems: string[] = [];
      const check = makeCheck(page, `${width}${scheme === "dark" ? "-dark" : ""}-text`, problems);
      const assets = makeAssets(30);
      const seen = { opened: [] as string[] };
      await mock(page, assets, { [assets[0]!.id]: READ, [assets[1]!.id]: EMPTY, [assets[2]!.id]: PENDING }, seen);

      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_/ }).first().click();
      await page.getByRole("dialog").waitFor();
      await check("viewer");

      await page.getByRole("button", { name: "Text" }).click();
      const panel = page.getByRole("complementary", { name: "Text in this photo" });
      await panel.waitFor();
      await expect(panel.getByLabel("Recognised text")).toContainText("HOFER KG");
      await expect(page.locator(".text-box:not(.qr)")).toHaveCount(4);
      await expect(page.locator(".text-box.qr")).toHaveCount(3);
      await check("text-panel");
      // The boxes sit over the photo: inside the picture's own rectangle.
      const inside = await page.evaluate(() => {
        const img = document.querySelector<HTMLImageElement>(".lightbox-stage img")!.getBoundingClientRect();
        const all = Array.from(document.querySelectorAll<HTMLElement>(".text-box")).map((b) => b.getBoundingClientRect());
        const bad = all.filter((r) => r.left < img.left - 1 || r.right > img.right + 1 || r.top < img.top - 1 || r.bottom > img.bottom + 1);
        return { bad: bad.length, img: [img.left, img.top, img.right, img.bottom].map(Math.round), first: bad[0] ? [bad[0].left, bad[0].top, bad[0].right, bad[0].bottom].map(Math.round) : null };
      });
      expect(inside, JSON.stringify(inside)).toMatchObject({ bad: 0 });

      // QR codes: a link only for the web address, and nothing opened by itself.
      const codes = panel.getByRole("region", { name: "QR codes" });
      await codes.scrollIntoViewIfNeeded();
      await expect(codes.getByRole("link")).toHaveCount(1);
      await expect(codes.getByRole("link")).toHaveAttribute("rel", /noopener/);
      await expect(codes.getByText("javascript:alert(document.cookie)")).toBeVisible();
      await check("text-qr");
      expect(seen.opened).toEqual([]);

      // Boxes off, Escape closes the panel before the viewer; `t` brings it back.
      await panel.getByLabel("Show on the photo").uncheck();
      await expect(page.locator(".text-box")).toHaveCount(0);
      await panel.getByRole("heading", { name: "Text" }).click(); // out of the checkbox: typing in a field is not a viewer shortcut
      await page.keyboard.press("Escape");
      await expect(panel).toHaveCount(0);
      await page.keyboard.press("t");
      await panel.waitFor();
      await panel.getByLabel("Show on the photo").check();

      // Another photo: nothing found, then not read yet.
      await page.getByRole("button", { name: "Next photo" }).click();
      await expect(panel.getByText("No text found in this photo.")).toBeVisible();
      await check("text-empty");
      await page.getByRole("button", { name: "Next photo" }).click();
      await expect(panel.getByText(/has not been read yet/)).toBeVisible();
      await check("text-pending");

      // Info and Text are one panel at a time.
      await page.getByRole("button", { name: "Info" }).click();
      await expect(page.getByRole("complementary", { name: "Photo details" })).toBeVisible();
      await expect(panel).toHaveCount(0);

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test("the search filter 'Text in the photo' is in the sheet and makes a chip", async ({ page }) => {
      test.setTimeout(60_000);
      const problems: string[] = [];
      const check = makeCheck(page, `${width}${scheme === "dark" ? "-dark" : ""}-text`, problems);
      await mock(page, makeAssets(30), {}, { opened: [] });
      await page.goto("/search?has_text=true");
      await expect(page.getByText("Has text", { exact: true }).first()).toBeVisible();
      await check("search-has-text");
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
