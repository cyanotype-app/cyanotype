// The photo in the viewer fits: whatever the screen, the picture lies fully inside the stage (the area between the bar and
// the stars/caption), keeps its shape, and fills the stage's width or height. Phones, a short landscape phone, a tablet and
// desktops, light and dark, with the Info panel open on desktop. Set SHOT_DIR to save a screenshot per state.
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { SHOT_DIR } from "./kit";
import { SHAPES, installPictures, openPhoto, setupViewer } from "./viewerKit";

const SIZES = [
  { name: "phone-320x568", width: 320, height: 568, touch: true },
  { name: "phone-375x667", width: 375, height: 667, touch: true },
  { name: "phone-390x844", width: 390, height: 844, touch: true },
  { name: "phone-414x896", width: 414, height: 896, touch: true },
  { name: "landscape-phone-844x390", width: 844, height: 390, touch: true },
  { name: "tablet-768x1024", width: 768, height: 1024, touch: false },
  { name: "desktop-1280x720", width: 1280, height: 720, touch: false },
  { name: "desktop-1920x1080", width: 1920, height: 1080, touch: false },
];

/** The picture's box against the stage's and the screen's, and what lies under the stage. */
async function measure(page: Page) {
  return page.evaluate(() => {
    const r = (el: Element | null) => {
      const b = el?.getBoundingClientRect();
      return b ? { l: b.left, t: b.top, r: b.right, b: b.bottom, w: b.width, h: b.height } : null;
    };
    const img = document.querySelector<HTMLImageElement>(".lightbox-stage img");
    const lb = document.querySelector<HTMLElement>(".lightbox");
    return {
      img: r(img),
      natural: img ? ([img.naturalWidth, img.naturalHeight] as [number, number]) : null,
      stage: r(document.querySelector(".lightbox-stage")),
      bar: r(document.querySelector(".lightbox-bar")),
      rating: r(document.querySelector(".lightbox-rating")),
      caption: r(document.querySelector(".lightbox-caption")),
      vw: window.innerWidth,
      vh: window.innerHeight,
      lightboxOverflow: lb ? lb.scrollHeight - lb.clientHeight : 0,
      barOutside: Array.from(document.querySelectorAll<HTMLElement>('.lightbox-bar button, .lightbox-bar a'))
        .filter((e) => e.getBoundingClientRect().width > 0)
        .filter((e) => e.getBoundingClientRect().right > window.innerWidth + 0.5 || e.getBoundingClientRect().left < -0.5)
        .map((e) => (e.textContent || e.getAttribute('aria-label') || '').trim()),
      docOverflow: document.documentElement.scrollWidth - window.innerWidth,
    };
  });
}

async function expectFits(page: Page, label: string) {
  // The picture settles after the preview loads and the panels slide in: wait until two reads agree.
  let m = await measure(page);
  await expect
    .poll(async () => {
      const next = await measure(page);
      const same = JSON.stringify(next.img) === JSON.stringify(m.img) && JSON.stringify(next.stage) === JSON.stringify(m.stage);
      m = next;
      return same && !!next.img;
    }, { message: label })
    .toBe(true);
  const { img, stage, vw, vh } = m;
  if (!img || !stage || !m.natural) throw new Error(`${label}: no picture`);
  const tol = 1.5;
  expect(img.l, `${label}: left inside the stage`).toBeGreaterThanOrEqual(stage.l - tol);
  expect(img.r, `${label}: right inside the stage`).toBeLessThanOrEqual(stage.r + tol);
  expect(img.t, `${label}: top inside the stage`).toBeGreaterThanOrEqual(stage.t - tol);
  expect(img.b, `${label}: bottom inside the stage (${img.b} vs ${stage.b})`).toBeLessThanOrEqual(stage.b + tol);
  expect(img.r, `${label}: inside the screen`).toBeLessThanOrEqual(vw + tol);
  expect(img.b, `${label}: above the bottom of the screen`).toBeLessThanOrEqual(vh + tol);
  expect(stage.b, `${label}: the stage ends on screen`).toBeLessThanOrEqual(vh + tol);
  if (m.bar) expect(img.t, `${label}: not under the bar`).toBeGreaterThanOrEqual(m.bar.b - tol);
  if (m.rating) expect(img.b, `${label}: not under the stars`).toBeLessThanOrEqual(m.rating.t + tol);
  if (m.caption) expect(m.caption.b, `${label}: caption on screen`).toBeLessThanOrEqual(vh + tol);
  expect(m.barOutside, `${label}: the bar's buttons are on screen`).toEqual([]);
  expect(m.lightboxOverflow, `${label}: the viewer does not scroll`).toBeLessThanOrEqual(1);
  expect(m.docOverflow, `${label}: the page does not scroll sideways`).toBeLessThanOrEqual(1);
  // Fills as much as possible: touches the stage's width or height (unless it would be upscaled past 2x its pixels).
  const [nw, nh] = m.natural;
  const capped = img.w >= nw * 2 - 2;
  const touches = Math.abs(img.w - stage.w) <= 2 || Math.abs(img.h - stage.h) <= 2;
  expect(touches || capped, `${label}: fills the stage (image ${Math.round(img.w)}x${Math.round(img.h)} in ${Math.round(stage.w)}x${Math.round(stage.h)})`).toBe(true);
  // The shape is kept.
  expect(Math.abs(img.w / img.h - nw / nh) / (nw / nh), `${label}: aspect ratio`).toBeLessThan(0.01);
}

for (const scheme of ["light", "dark"] as const) {
  for (const size of SIZES) {
    test.describe(`viewer fit ${size.name} ${scheme}`, () => {
      test.use({ colorScheme: scheme, viewport: { width: size.width, height: size.height }, hasTouch: size.touch, isMobile: size.touch });

      test("every shape of photo fits the stage", async ({ page }) => {
        test.setTimeout(90_000);
        if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
        await setupViewer(page);
        await openPhoto(page, 0);
        for (let i = 0; i < SHAPES.length; i++) {
          const s = SHAPES[i]!;
          if (i > 0) {
            await page.keyboard.press("ArrowRight");
            await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", new RegExp(`/${s.name}\\.svg`));
          }
          await expectFits(page, `${size.name} ${scheme} ${s.name}`);
          if (SHOT_DIR) await page.screenshot({ path: join(SHOT_DIR, `fit-${size.name}-${scheme}-${s.name}.png`) });
        }
      });

      if (size.width >= 768) {
        test("with the Info panel open", async ({ page }) => {
          test.setTimeout(90_000);
          await setupViewer(page);
          await openPhoto(page, 0);
          await page.keyboard.press("i");
          await expect(page.locator("#details-panel")).toBeVisible();
          for (let i = 0; i < SHAPES.length; i++) {
            if (i > 0) {
              await page.keyboard.press("ArrowRight");
              await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", new RegExp(`/${SHAPES[i]!.name}\\.svg`));
            }
            await expectFits(page, `${size.name} ${scheme} info ${SHAPES[i]!.name}`);
          }
          if (SHOT_DIR) await page.screenshot({ path: join(SHOT_DIR, `fit-${size.name}-${scheme}-info.png`) });
        });
      }
    });
  }
}

/** The public share page's viewer is the same stage: its pictures fit too. */
for (const size of [SIZES[0]!, SIZES[2]!, SIZES[4]!, SIZES[6]!]) {
  test.describe(`share viewer fit ${size.name}`, () => {
    test.use({ viewport: { width: size.width, height: size.height }, hasTouch: size.touch, isMobile: size.touch });

    test("every shape of photo fits the stage", async ({ page }) => {
      await installPictures(page, 3);
      const items = SHAPES.map((s, i) => ({
        id: `p${i}`,
        width: s.w,
        height: s.h,
        sort_time: "2026-10-14T10:00:00Z",
        thumbhash: null,
        thumb_url: `/fixture-preview/${s.name}.svg`,
        preview_url: `/fixture-preview/${s.name}.svg`,
        downloads: [],
        download_pending: false,
      }));
      await page.route("**/assets/public/s/open", (r) =>
        r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ needs_password: false, unlocked: true, kind: "selection", title: null, item_count: items.length, allow_download: false, include_originals: false, expires_at: null }) }),
      );
      await page.route(/\/assets\/public\/s\/open\/items(\?.*)?$/, (r) =>
        r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items, next_cursor: null }) }),
      );
      await page.goto("/s/open");
      await page.getByRole("button", { name: /Open photo 1 of/ }).click();
      await page.locator(".lightbox-stage img.loaded").waitFor();
      for (let i = 0; i < SHAPES.length; i++) {
        if (i > 0) {
          await page.keyboard.press("ArrowRight");
          await expect(page.locator(".lightbox-stage img.loaded")).toHaveAttribute("src", new RegExp(`/${SHAPES[i]!.name}\\.svg`));
        }
        await expectFits(page, `share ${size.name} ${SHAPES[i]!.name}`);
      }
    });
  });
}
