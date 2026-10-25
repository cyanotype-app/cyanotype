// Shared by the slideshow and photo-frame specs: a mocked server for the ordered feed (`/assets/slideshow`) and the preview
// addresses (`/assets/previews`), pictures of mixed shapes on the bucket's own origin (so the gateway's real CSP is in
// force), and a watcher that fails the test on any Content-Security-Policy violation.
import type { Page } from "@playwright/test";
import { json, type Handler } from "./kit";
import { TEST_S3_ORIGIN } from "./csp";
import type { FakeAsset } from "./fakeLibrary";

/** Landscape, portrait, panorama and square pictures, so the fit is checked for every shape. */
const SHAPES = [
  { w: 1600, h: 1000 },
  { w: 1000, h: 1600 },
  { w: 1800, h: 450 },
  { w: 1200, h: 1200 },
];
const palette = ["#1d4e7a", "#2f7cb5", "#5c9bcb", "#0a4a78", "#8fbbe0", "#14609a"];
export const shapeOf = (n: number) => SHAPES[n % SHAPES.length]!;

export const slidePicture = (n: number) => {
  const { w, h } = shapeOf(n);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><rect width="${w}" height="${h}" fill="${palette[n % palette.length]}"/><circle cx="${w / 2}" cy="${h / 2}" r="${Math.min(w, h) / 3}" fill="#fff" fill-opacity=".35"/><text x="${w / 2}" y="${h / 2}" font-size="${Math.min(w, h) / 6}" text-anchor="middle" fill="#000">${n}</text></svg>`;
};

export interface SlideServer {
  /** Query strings of every `/assets/slideshow` request. */
  feed: string[];
  /** Ids asked for in `/assets/previews`. */
  previews: string[];
  /** While true the server "is unreachable": the feed and the previews abort like a dropped connection. */
  down: boolean;
  /** While true the feed and previews answer 401 (the session has ended). */
  expired: boolean;
  /** Ids of photos the server leaves out (not rendered yet, gone). */
  missing: Set<string>;
}

const hash = (s: string) => {
  let h = 2166136261;
  for (const c of s) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return h >>> 0;
};

/** The feed and previews of a library `assets` (newest first). Add the returned handler to `setupApp({ handlers })`. */
export function slideServer(assets: () => FakeAsset[]) {
  const state: SlideServer = { feed: [], previews: [], down: false, expired: false, missing: new Set() };
  const handler: Handler = (route, url, method) => {
    const p = url.pathname;
    if (method !== "GET" || (p !== "/assets/slideshow" && p !== "/assets/previews")) return false;
    if (state.down) {
      void route.abort("connectionfailed");
      return true;
    }
    if (state.expired) {
      void route.fulfill(json({ error: "Not signed in" }, 401));
      return true;
    }
    const all = assets();
    if (p === "/assets/slideshow") {
      state.feed.push(url.search);
      const order = url.searchParams.get("order") ?? "listed";
      const seed = url.searchParams.get("seed") ?? "";
      const limit = Number(url.searchParams.get("limit") ?? 40);
      const at = Number(url.searchParams.get("cursor") ?? 0);
      const ordered =
        order === "oldest" ? all.slice().reverse() : order === "shuffle" ? all.slice().sort((a, b) => hash(seed + a.id) - hash(seed + b.id)) : all;
      const items = ordered.slice(at, at + limit);
      void route.fulfill(json({ items, next_cursor: at + limit < ordered.length ? String(at + limit) : null, total: ordered.length }));
      return true;
    }
    const ids = (url.searchParams.get("ids") ?? "").split(",").filter(Boolean);
    state.previews.push(...ids);
    void route.fulfill(
      json({
        items: ids
          .filter((id) => !state.missing.has(id))
          .map((id) => {
            const n = Number(id.slice(1));
            const { w, h } = shapeOf(n);
            return { id, preview_url: `${TEST_S3_ORIGIN}/fixture-slide/${n}.svg`, width: w, height: h, caption: n % 3 === 0 ? `Photo ${n}: a long caption about the day by the lake` : null };
          }),
      }),
    );
    return true;
  };
  return { state, handler };
}

/** Serve the pictures the previews point at (on the bucket origin, as presigned URLs are). */
export async function installSlidePictures(page: Page) {
  await page.route(`${TEST_S3_ORIGIN}/fixture-slide/*.svg`, (r) =>
    r.fulfill({ status: 200, contentType: "image/svg+xml", headers: { "access-control-allow-origin": "*" }, body: slidePicture(Number(/(\d+)\.svg/.exec(r.request().url())![1])) }),
  );
}

/** Collect every CSP violation the page reports. */
export async function watchViolations(page: Page) {
  const seen: string[] = [];
  await page.exposeFunction("__reportViolation", (s: string) => seen.push(s));
  await page.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (e) => {
      void (window as unknown as { __reportViolation: (s: string) => void }).__reportViolation(
        `${e.violatedDirective} blocked ${e.blockedURI || "inline"} sample=${e.sample}`,
      );
    });
  });
  page.on("console", (m) => {
    if (/Content Security Policy|Content-Security-Policy/i.test(m.text())) seen.push(`console: ${m.text()}`);
  });
  return seen;
}

/** The visible photo layer's picture box against the screen. */
export async function pictureBox(page: Page, root: string) {
  return page.evaluate((sel) => {
    const layer = Array.from(document.querySelectorAll<HTMLElement>(`${sel} .slide-layer`)).at(-1);
    const img = layer?.querySelector<HTMLImageElement>(".slide-photo");
    if (!img) return null;
    // The drawn picture inside an object-fit: contain box.
    const b = img.getBoundingClientRect();
    const nw = img.naturalWidth;
    const nh = img.naturalHeight;
    const scale = Math.min(b.width / nw, b.height / nh);
    const w = nw * scale;
    const h = nh * scale;
    const left = b.left + (b.width - w) / 2;
    const top = b.top + (b.height - h) / 2;
    return { left, top, right: left + w, bottom: top + h, w, h, nw, nh, vw: window.innerWidth, vh: window.innerHeight, src: img.getAttribute("src") ?? "" };
  }, root);
}
