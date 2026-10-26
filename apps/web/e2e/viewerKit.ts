// Shared by the viewer's fit and zoom specs: test photos of known shapes served as SVG (so a 1440 px preview costs
// nothing), and the detail answers that point the viewer at them.
import type { Page } from "@playwright/test";
import { json, setupApp } from "./kit";

export interface Shape {
  name: string;
  w: number;
  h: number;
}

/** What the viewer is shown, in the order of the library's first photos (a0, a1, ...). */
export const SHAPES: Shape[] = [
  { name: "landscape", w: 1440, h: 960 },
  { name: "portrait", w: 960, h: 1440 },
  { name: "panorama", w: 1440, h: 360 },
  { name: "huge", w: 4000, h: 3000 },
];

const picture = (w: number, h: number, label: string) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><rect width="${w}" height="${h}" fill="#2f7cb5"/><rect x="0" y="0" width="${w / 2}" height="${h / 2}" fill="#14609a"/><circle cx="${w / 2}" cy="${h / 2}" r="${Math.min(w, h) / 4}" fill="#fff" fill-opacity=".4"/><text x="${w / 2}" y="${h / 2}" font-size="${Math.min(w, h) / 8}" text-anchor="middle" fill="#000">${label}</text></svg>`;

export interface ViewerOptions {
  /** Called for every request of a photo's larger rendition (`/fixture-original/<name>.svg`). */
  onOriginal?: (name: string) => void;
  /** How many times larger the "original" is than the preview; default 3. */
  originalScale?: number;
  /** Ids of photos that have an edit recipe (no sharper rendition is offered for them). */
  edited?: string[];
  /** Content type of every photo (default image/jpeg). */
  contentType?: string;
  count?: number;
}

/** The preview and original files of the SHAPES (the original is `scale` times as large). */
export async function installPictures(page: Page, scale: number, onOriginal?: (name: string) => void) {
  await page.route("**/fixture-preview/*.svg", (r) => {
    const s = SHAPES.find((x) => r.request().url().includes(`/${x.name}.svg`))!;
    return r.fulfill({ status: 200, contentType: "image/svg+xml", body: picture(s.w, s.h, s.name) });
  });
  await page.route("**/fixture-original/*.svg", (r) => {
    const s = SHAPES.find((x) => r.request().url().includes(`/${x.name}.svg`))!;
    onOriginal?.(s.name);
    return r.fulfill({ status: 200, contentType: "image/svg+xml", body: picture(s.w * scale, s.h * scale, `${s.name} original`) });
  });
}

/**
 * A library whose first photos are the SHAPES: the preview is the shape's own size, `original_url` is a file three times
 * as large (the viewer may swap to it past the preview's resolution), `width`/`height` are the original's.
 */
export async function setupViewer(page: Page, opts: ViewerOptions = {}) {
  const scale = opts.originalScale ?? 3;
  const assets = await setupApp(page, { count: opts.count ?? 8 });
  await installPictures(page, scale, opts.onOriginal);
  const shapeOf = (i: number) => SHAPES[i % SHAPES.length]!;
  assets.forEach((a, i) => {
    const s = shapeOf(i);
    a.width = s.w * scale;
    a.height = s.h * scale;
    if (opts.contentType) a.content_type = opts.contentType;
    if (opts.edited?.includes(a.id)) (a as unknown as { edited: boolean }).edited = true;
  });
  await page.route(/\/assets\/a\d+$/, async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    const id = /\/assets\/(a\d+)$/.exec(new URL(route.request().url()).pathname)![1]!;
    const a = assets.find((x) => x.id === id);
    if (!a) return route.fallback();
    const s = shapeOf(Number(id.slice(1)));
    const edited = opts.edited?.includes(id) ?? false;
    return route.fulfill(
      json({
        ...a,
        albums: [],
        original_url: `/fixture-original/${s.name}.svg`,
        preview_url: `/fixture-preview/${s.name}.svg`,
        thumb_url: `/fixture-preview/${s.name}.svg`,
        recipe: edited ? { rotate: 90 } : null,
        rendering: false,
        file_width: s.w * scale,
        file_height: s.h * scale,
      }),
    );
  });
  return assets;
}

/** Open photo `index` (0-based) of the library in the viewer and wait for its picture. */
export async function openPhoto(page: Page, index = 0) {
  await page.goto("/library");
  await page.getByRole("button", { name: new RegExp(`Open IMG_${String(index).padStart(5, "0")}`) }).first().click();
  const viewer = page.getByRole("dialog");
  await viewer.waitFor();
  // The preview (not the thumbnail shown while it loads), laid out in the box its own shape gives it.
  await viewer.locator(".viewer-photo.current img.loaded.sized[src*='fixture-preview']").waitFor();
  await page.waitForFunction(() => {
    const i = document.querySelector<HTMLImageElement>(".viewer-photo.current img");
    return !!i && i.naturalWidth >= 960 && i.style.getPropertyValue("--ar") !== "";
  });
  return viewer;
}
