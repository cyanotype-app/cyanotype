// A fake of the assets API's timeline, list, favorite, detail and download endpoints for the browser tests, with a
// library of any size spread over several years (days with dozens of photos, weeks with none).
import type { Page, Route } from "@playwright/test";
import { createServer } from "node:http";

export interface FakeAsset {
  id: string;
  original_name: string;
  content_type: string;
  size_bytes: number;
  width: number;
  height: number;
  sort_time: string;
  thumbhash: null;
  thumb_url: string;
  favorite: boolean;
  paired: { id: string; original_name: string; content_type: string; size_bytes: number; kind: "raw" }[];
}

const palette = ["#1d4e7a", "#2f7cb5", "#5c9bcb", "#0a4a78", "#8fbbe0", "#14609a"];
export const svg = (n: number) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300"><rect width="300" height="300" fill="${palette[n % palette.length]}"/><circle cx="150" cy="150" r="${40 + (n % 5) * 15}" fill="#fff" fill-opacity=".35"/></svg>`;

/** Newest first, from 14 Oct 2026 backwards: bursts of photos minutes apart, now and then a gap of days. */
export function makeAssets(count: number, { yearsBack = 7 }: { yearsBack?: number } = {}): FakeAsset[] {
  let seed = 12345;
  const rand = () => (seed = (seed * 1664525 + 1013904223) % 4294967296) / 4294967296;
  const end = Date.UTC(2026, 9, 14, 21, 0, 0);
  const span = yearsBack * 365 * 86_400_000;
  const meanGap = span / count;
  const out: FakeAsset[] = [];
  let t = end;
  for (let i = 0; i < count; i++) {
    // 85%: minutes apart (a shoot); 15%: a long gap, sized so the average spreads the library over `yearsBack` years.
    t -= rand() < 0.85 ? 60_000 + rand() * 25 * 60_000 : (meanGap - 0.85 * 13.5 * 60_000) / 0.15 * (0.5 + rand());
    const raw = i % 7 === 0;
    out.push({
      id: `a${i}`,
      original_name: `IMG_${String(i).padStart(5, "0")}.jpg`,
      content_type: "image/jpeg",
      size_bytes: 3_000_000,
      width: 4000,
      height: 3000,
      sort_time: new Date(t).toISOString(),
      thumbhash: null,
      thumb_url: `/fixture-thumbs/${i}.svg`,
      favorite: i % 11 === 3,
      paired: raw
        ? [{ id: `r${i}`, original_name: `IMG_${String(i).padStart(5, "0")}.CR3`, content_type: "image/x-canon-cr3", size_bytes: 25_000_000, kind: "raw" }]
        : [],
    });
  }
  return out;
}

export const dayOf = (iso: string) => iso.slice(0, 10);

export function timelineOf(assets: FakeAsset[], favorite: boolean) {
  const days: { day: string; count: number }[] = [];
  for (const a of assets) {
    if (favorite && !a.favorite) continue;
    const d = dayOf(a.sort_time);
    const last = days.at(-1);
    if (last?.day === d) last.count++;
    else days.push({ day: d, count: 1 });
  }
  return { total: days.reduce((n, d) => n + d.count, 0), days };
}

/** `assets` is sorted newest first; `before` is strict, `cursor` is `<index>`. */
export function pageOf(assets: FakeAsset[], q: URLSearchParams) {
  const limit = Number(q.get("limit") ?? 100);
  const favorite = q.get("favorite") === "true";
  let from = 0;
  const cursor = q.get("cursor");
  const before = q.get("before");
  if (cursor) from = Number(cursor);
  else if (before) {
    const t = Date.parse(before);
    let lo = 0;
    let hi = assets.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (Date.parse(assets[mid]!.sort_time) < t) hi = mid;
      else lo = mid + 1;
    }
    from = lo;
  }
  const items: FakeAsset[] = [];
  let i = from;
  for (; i < assets.length && items.length < limit; i++) if (!favorite || assets[i]!.favorite) items.push(assets[i]!);
  const more = i < assets.length && items.length === limit;
  return { items, next_cursor: more ? String(i) : null, next_before: null };
}

export interface BucketRequest {
  url: string;
  /** `Sec-Fetch-Mode`: `navigate` for a link click, `cors` for fetch()/XHR. */
  mode: string;
  dest: string;
}

/**
 * A stand-in for the bucket on its own origin (another port), like S3: it sends the original with whatever
 * `response-content-disposition` the presigned URL asked for, and no CORS headers at all.
 */
export async function startBucket() {
  const requests: BucketRequest[] = [];
  const server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://bucket");
    requests.push({ url: req.url ?? "", mode: String(req.headers["sec-fetch-mode"]), dest: String(req.headers["sec-fetch-dest"]) });
    res.writeHead(200, {
      "content-type": "application/octet-stream",
      "content-disposition": url.searchParams.get("response-content-disposition") ?? "inline",
    });
    res.end("fake original bytes");
  });
  await new Promise<void>((ok) => server.listen(0, "127.0.0.1", ok));
  const { port } = server.address() as { port: number };
  return {
    origin: `http://127.0.0.1:${port}`,
    requests,
    close: () => new Promise<void>((ok) => server.close(() => ok())),
  };
}

export interface Requests {
  timeline: string[];
  list: string[];
  favorites: { ids: string[]; favorite: boolean }[];
}

/** Handle the endpoints above (everything else under /assets and /auth is left to `fallback`). */
export async function installLibrary(
  page: Page,
  assets: FakeAsset[],
  fallback: (route: Route) => Promise<void> | void,
  bucketOrigin = "http://127.0.0.1:1",
): Promise<Requests> {
  const seen: Requests = { timeline: [], list: [], favorites: [] };
  const json = (body: unknown) => ({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/fixture-thumbs/*.svg", (r) =>
    r.fulfill({ status: 200, contentType: "image/svg+xml", body: svg(Number(/(\d+)\.svg/.exec(r.request().url())![1])) }),
  );
  await page.route(
    (u) => /^\/assets(\/|$)/.test(u.pathname),
    async (route) => {
      const req = route.request();
      const u = new URL(req.url());
      const p = u.pathname;
      if (req.method() === "GET" && p === "/assets/processing") {
        return route.fulfill(
          json({ active: false, queued: 0, running: 0, failed: 0, by_kind: [], total_photos: assets.length, without_thumbnail: 0, failures: [] }),
        );
      }
      if (req.method() === "GET" && p === "/assets/timeline") {
        seen.timeline.push(u.search);
        return route.fulfill(json(timelineOf(assets, u.searchParams.get("favorite") === "true")));
      }
      if (req.method() === "GET" && p === "/assets") {
        seen.list.push(u.search);
        return route.fulfill(json(pageOf(assets, u.searchParams)));
      }
      if (req.method() === "POST" && p === "/assets/favorite") {
        const body = req.postDataJSON() as { ids: string[]; favorite: boolean };
        seen.favorites.push(body);
        let count = 0;
        for (const a of assets) {
          if (body.ids.includes(a.id) && a.favorite !== body.favorite) {
            a.favorite = body.favorite;
            count++;
          }
        }
        return route.fulfill(json({ count }));
      }
      const dl = /^\/assets\/([^/]+)\/download$/.exec(p);
      if (req.method() === "GET" && dl) {
        const file = u.searchParams.get("file");
        const a = assets.find((x) => x.id === dl[1]);
        const name = file ? a?.paired.find((x) => x.id === file)?.original_name : a?.original_name;
        if (!name) return route.fulfill({ status: 404, contentType: "application/json", body: '{"error":"not found"}' });
        return route.fulfill({
          status: 307,
          headers: {
            location: `${bucketOrigin}/originals/u/${name}?response-content-disposition=${encodeURIComponent(`attachment; filename="${name}"`)}&X-Amz-Expires=60&X-Amz-Signature=abc`,
          },
        });
      }
      const one = /^\/assets\/(a\d+)$/.exec(p);
      if (req.method() === "GET" && one) {
        const a = assets.find((x) => x.id === one[1]);
        if (!a) return route.fulfill({ status: 404, contentType: "application/json", body: '{"error":"not found"}' });
        return route.fulfill(
          json({ ...a, albums: [], original_url: a.thumb_url, preview_url: a.thumb_url }),
        );
      }
      return fallback(route);
    },
  );
  return seen;
}
