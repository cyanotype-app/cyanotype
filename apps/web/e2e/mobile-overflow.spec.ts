// Renders every page and overlay with mocked API data at phone widths and asserts the page never scrolls
// sideways. Set SHOT_DIR to also save a screenshot of each state at every width.
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { installLibrary, makeAssets, pageOf, timelineOf, type FakeAsset } from "./fakeLibrary";

const SHOT_DIR = process.env.SHOT_DIR;
// A real Protomaps archive for zoom 0-2 (the world), served with range requests like the assets service does.
const WORLD_TILES = readFileSync(new URL("./fixtures/world-z2.pmtiles", import.meta.url));
let basemapInstalled = true;
const RUNS: { width: number; scheme: "light" | "dark" }[] = [
  ...[320, 360, 375, 390, 414, 1280].map((width) => ({ width, scheme: "light" as const })),
  { width: 360, scheme: "dark" },
];
const LONG_ALBUM = "Summer road trip along the entire Pacific coast highway with the whole extended family, 2019 to 2024";
const LONG_NAME = "IMG_20240615_184233_a-really-quite-long-camera-generated-filename-with-no-spaces-at-all-HDR-PANO.jpg";

const palette = ["#1d4e7a", "#2f7cb5", "#5c9bcb", "#0a4a78", "#8fbbe0", "#14609a"];
const svg = (n: number) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300"><rect width="300" height="300" fill="${palette[n % palette.length]}"/><circle cx="150" cy="150" r="${40 + (n % 5) * 15}" fill="#fff" fill-opacity=".35"/></svg>`;
const thumb = (n: number) => `/fixture-thumbs/${n}.svg`;
// Markers of the map page: single photos, clusters of every size class, and a long count.
const marker = (id: string, lat: number, lon: number, count: number, n: number) => ({
  id,
  lat,
  lon,
  count,
  bounds: count > 1 ? [lon - 0.4, lat - 0.3, lon + 0.4, lat + 0.3] : null,
  thumbhash: null,
  thumb_url: count === 1 ? thumb(n) : null,
});
const MAP_MARKERS = [
  marker("a1", 30.04, 31.24, 1, 1),
  marker("a2", 51.5, -0.12, 7, 2),
  marker("a3", 40.7, -74.0, 128, 3),
  marker("a4", 35.68, 139.65, 12400, 4),
  marker("a5", -33.86, 151.2, 1, 5),
  marker("a6", 37.77, -122.42, 2, 6),
];

const asset = (n: number) => ({
  id: `a${n}`,
  original_name: n === 2 ? LONG_NAME : `IMG_${1000 + n}.jpg`,
  content_type: "image/jpeg",
  size_bytes: 3_000_000,
  width: 4000,
  height: 3000,
  sort_time: new Date(Date.UTC(2024, 5, 30 - n, 12)).toISOString(),
  thumbhash: null,
  thumb_url: thumb(n),
});
const assets = Array.from({ length: 30 }, (_, i) => asset(i));
const trash = assets.slice(0, 9).map((a) => ({
  ...a,
  deleted_at: new Date(Date.now() - 2 * 86_400_000).toISOString(),
  purge_at: new Date(Date.now() + 28 * 86_400_000).toISOString(),
}));
const album = (id: string, title: string, count: number, withDesc = false) => ({
  id,
  title,
  description: withDesc ? "A long description. ".repeat(12) + "x".repeat(60) : null,
  photo_count: count,
  cover: { id: "a1", thumbhash: null, thumb_url: thumb(1) },
  first_at: "2019-01-01T00:00:00Z",
  last_at: "2024-06-30T00:00:00Z",
  created_at: "2024-01-01T00:00:00Z",
  updated_at: "2024-01-01T00:00:00Z",
});
const albums = [
  album("l1", LONG_ALBUM, 30, true),
  album("l2", "Pets", 12),
  album("l3", "Averyveryveryveryveryveryveryveryveryveryveryverylongsinglewordalbumtitle", 3),
  album("l4", "Garden", 7),
];

const IDLE = { active: false, queued: 0, running: 0, failed: 0, by_kind: [], total_photos: 30, without_thumbnail: 0, failures: [] };
const LONG_ERROR =
  "decode refused: original is 187 MB, which does not fit the worker memory budget of 640 MB (a-very-long-unbroken-error-token-" + "x".repeat(120) + ")";
const BUSY = {
  ...IDLE,
  active: true,
  queued: 41234,
  running: 2,
  total_photos: 52000,
  without_thumbnail: 20617,
  by_kind: [{ kind: "thumbnail", queued: 20617, running: 2, failed: 3 }],
  failed: 1234,
  failures: [
    { job_id: 1, asset_id: "a2", original_name: LONG_NAME, kind: "thumbnail", last_error: LONG_ERROR, attempts: 5 },
    { job_id: 2, asset_id: "a3", original_name: "IMG_1003.CR3", kind: "metadata", last_error: null, attempts: 1 },
    { job_id: 3, asset_id: "a4", original_name: "IMG_1004.HEIC", kind: "thumbnail", last_error: "sha256 mismatch", attempts: 5 },
  ],
};
const INDEXING = { ...IDLE, total_photos: 52000, embedding: { indexed: 12345, total: 52000, queued: 39654, running: 1, failed: 0 } };
// An older model answers while the improved one indexes the library.
const REINDEXING = {
  ...IDLE,
  total_photos: 52000,
  embedding: {
    indexed: 20800, total: 52000, queued: 31199, running: 1, failed: 0,
    model: "siglip2-b16", percent: 40, reindexing: true, switch_at_percent: 95, serving_model: "clip-b32",
  },
};
let memberView = false;
let anonymous = false;
let processing: typeof IDLE | typeof BUSY | typeof INDEXING | typeof REINDEXING = IDLE;

// Search: the mock answers by what the query says. "nothing" matches no photo, "unavailable" is a server without the
// model; anything else ranks the library's photos. Facets carry long names to stress the filter sheet.
const LONG_CAMERA = "Hasselblad X2D 100C with the XCD 4/45P lens and the optional extension tube set";
const LONG_PLACE = "Llanfairpwllgwyngyllgogerychwyrndrobwllllantysiliogogogoch";
const FACETS = {
  total: 52000,
  favorites: 412,
  with_location: 30011,
  types: [{ value: "jpeg", count: 41000 }, { value: "heic", count: 9000 }, { value: "raw", count: 6000 }],
  cameras: [{ value: LONG_CAMERA, count: 5230 }, { value: "Canon EOS R5", count: 30100 }, { value: "Apple iPhone 15 Pro Max", count: 16670 }],
  places: [
    { value: "United Kingdom", kind: "country", count: 3000 },
    { value: LONG_PLACE, kind: "city", count: 12 },
    { value: "Lisbon", kind: "city", count: 4100 },
  ],
  years: [{ value: "2024", count: 9000 }, { value: "2023", count: 12000 }],
};
const searchHits = (n: number, weak = 0) =>
  assets.slice(0, n + weak).map((a, i) => ({ ...a, score: 0.31 - i * 0.004, probability: 0.1, tier: i < n ? "strong" : "weak" }));
// Results with bursts: the 2nd and the 5th result are covers of bursts of 4 and 12 photos (members are photos the list
// does not show otherwise), so the badge, the open burst and the selection note have their widest content.
const burstHits = () =>
  searchHits(10).map((h, i) =>
    i === 1 || i === 4
      ? { ...h, burst: { size: i === 1 ? 4 : 12, members: assets.slice(12, 12 + (i === 1 ? 3 : 11)).map((a) => ({ ...a, score: 0.2 })) } }
      : h,
  );
// What the server reads out of a query: a long place and a date, so the chips are as wide as they get.
const PARSED = {
  text: "dog",
  chips: [
    { kind: "date", label: "Summer 2022", matched: "in summer 2022", from: "2022-06-01", to: "2022-08-31" },
    { kind: "place", label: LONG_PLACE, matched: "in " + LONG_PLACE, value: LONG_PLACE, place_kind: "city" },
  ],
};

// The photo details panel: every row at its widest (long camera, lens, place, software and file names, a RAW pair, a burst
// and several albums), and, for the other photos, nothing but the file.
const LONG_LENS = "Canon RF 100-500mm F4.5-7.1 L IS USM with the 1.4x extender and the tripod collar attached";
const LONG_SOFTWARE = "Adobe Photoshop Lightroom Classic 13.2 (Macintosh) with a rather long-winded processing history";
const fullDetails = (id: string) => ({
  id,
  date: { instant: "2023-07-04T11:05:09Z", local: "2023-07-04T13:05:09", offset: "+02:00", zone_known: true, weekday: "Tuesday", basis: "taken", timeline_day: "2023-07-04" },
  location: {
    latitude: 38.7223,
    longitude: -9.1393,
    place: { city: LONG_PLACE, region: "Lisbon", country: "Portugal", country_code: "PT" },
    altitude: "38 m above sea level",
    altitude_m: 37.8,
    direction: "132° SE (magnetic)",
    direction_deg: 132.4,
  },
  camera: { label: LONG_CAMERA, make: "Hasselblad", model: "X2D", lens: LONG_LENS },
  exposure: {
    line: "ISO 12800 · 1000 mm · ƒ/22 · 1/8000 s · −0.7 ev",
    iso: "ISO 12800",
    focal_length: "1000 mm",
    focal_length_35mm: "1500 mm",
    aperture: "ƒ/22",
    shutter: "1/8000 s",
    bias: "−0.7 ev",
    flash: "Fired, auto, red-eye reduction",
    white_balance: "Manual",
    metering: "Center-weighted average",
    program: "Aperture priority",
  },
  file: { name: LONG_NAME, format: "JPEG", raw: false, content_type: "image/jpeg", size_bytes: 3_000_000, width: 11648, height: 8736, megapixels: "102 MP", color_space: "sRGB", software: LONG_SOFTWARE },
  files: [
    { id, name: LONG_NAME, format: "JPEG", raw: false, size_bytes: 3_000_000 },
    { id: "raw-" + id, name: LONG_NAME.replace(".jpg", ".CR3"), format: "CR3", raw: true, size_bytes: 25_000_000 },
  ],
  albums: [
    { id: "l1", title: LONG_ALBUM },
    { id: "l2", title: "Pets" },
    { id: "l3", title: albums[2]!.title },
  ],
  burst: { position: 3, size: 12 },
  caption: "A caption that goes on for a good while, with a_very_long_unbroken_word_" + "x".repeat(80) + " and then some more words after it.",
  edits: { date_edited: true, location_edited: true, original_date: { ...{ instant: "2023-07-04T11:05:09Z", local: "2023-07-04T13:05:09", offset: "+02:00", zone_known: true, weekday: "Tuesday", basis: "taken", timeline_day: "2023-07-04" } }, original_location: { latitude: 38.7, longitude: -9.1 } },
});
const bareDetails = (id: string) => ({
  ...fullDetails(id),
  date: { instant: "2024-02-03T04:05:06Z", local: "2024-02-03T04:05:06", offset: null, zone_known: false, weekday: "Saturday", basis: "file", timeline_day: "2024-02-03" },
  location: null,
  camera: null,
  exposure: null,
  file: { name: "scan.png", format: "PNG", raw: false, content_type: "image/png", size_bytes: 1000, width: null, height: null, megapixels: null, color_space: null, software: null },
  files: [{ id, name: "scan.png", format: "PNG", raw: false, size_bytes: 1000 }],
  albums: [],
  burst: null,
  caption: null,
  edits: { date_edited: false, location_edited: false, original_date: null, original_location: null },
});

async function mock(page: Page) {
  const json = (body: unknown) => ({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/fixture-thumbs/*.svg", (r) =>
    r.fulfill({ status: 200, contentType: "image/svg+xml", body: svg(Number(/(\d+)\.svg/.exec(r.request().url())![1])) }),
  );
  await page.route(
    (u) => /^\/(auth|assets)(\/|$)/.test(u.pathname),
    async (route) => {
      const req = route.request();
      const u = new URL(req.url());
      const p = u.pathname;
      const m = req.method();
      if (p === "/assets/tiles" && m === "GET") return route.fulfill(json({ world: basemapInstalled, detail: false }));
      if (p === "/assets/tiles/world.pmtiles") {
        const r = /bytes=(\d+)-(\d+)/.exec(req.headers()["range"] ?? "");
        if (!basemapInstalled || !r) return route.fulfill({ status: basemapInstalled ? 400 : 404, body: "" });
        const first = Number(r[1]);
        const last = Math.min(Number(r[2]), WORLD_TILES.length - 1);
        return route.fulfill({
          status: 206,
          contentType: "application/octet-stream",
          headers: { "content-range": `bytes ${first}-${last}/${WORLD_TILES.length}`, "accept-ranges": "bytes", etag: '"fixture"' },
          body: WORLD_TILES.subarray(first, last + 1),
        });
      }
      if (p === "/assets/map" && m === "GET") {
        const whole = u.searchParams.get("bbox") === "-180,-90,180,90";
        return route.fulfill(json({ clusters: whole ? MAP_MARKERS : MAP_MARKERS.slice(0, 4), total: whole ? 12632 : 12600, truncated: false }));
      }
      if (p === "/auth/me" && anonymous) return route.fulfill({ status: 401, contentType: "application/json", body: '{"error":"unauthorized"}' });
      if (p === "/auth/me") return route.fulfill(json({ id: "u1", email: "someone.with.a.long.address@example-photo-library.org", role: memberView ? "member" : "admin" }));
      if (p === "/auth/api-keys" && m === "GET")
        return route.fulfill(
          json({
            items: [
              { id: "k1", name: "laptop", prefix: "cy_abcd", created_at: "2024-06-01T00:00:00Z", last_used_at: null },
              { id: "k2", name: "A key with a really rather long descriptive name for the nightly import job", prefix: "cy_efgh", created_at: "2024-06-01T00:00:00Z", last_used_at: "2024-06-20T00:00:00Z" },
            ],
          }),
        );
      if (p === "/auth/api-keys" && m === "POST")
        return route.fulfill(
          json({ id: "k3", name: "new", prefix: "cy_zzzz", created_at: "2024-06-01T00:00:00Z", key: "cy_" + "0123456789abcdef".repeat(5) }),
        );
      if (p === "/assets/search/facets" && m === "GET") return route.fulfill(json(FACETS));
      if (p === "/assets/places" && m === "GET")
        return route.fulfill(
          json([
            { city: LONG_PLACE, region: "Wales", country: "United Kingdom", country_code: "GB", latitude: 53.2, longitude: -4.2, count: 12 },
            { city: "Lisbon", region: "Lisbon", country: "Portugal", country_code: "PT", latitude: 38.7, longitude: -9.1, count: 4100 },
          ]),
        );
      if (p === "/assets/edits" && m === "POST") return route.fulfill(json({ updated: 1, unchanged: 0, missing: 0, failed: [] }));
      if (p === "/assets/search" && m === "GET") {
        const q = u.searchParams.get("q") ?? "";
        if (q.includes("burst"))
          return route.fulfill(json({ items: burstHits(), next_cursor: null, ranked: true, total: null, strong_count: 10, weaker: null, parsed: { text: q, chips: [] } }));
        if (q.includes("unavailable"))
          return route.fulfill({ status: 503, contentType: "application/json", body: '{"error":"search is not available: the model files are not installed"}' });
        const weaker = u.searchParams.get("weaker") === "true";
        // "nothing": no strong match and some weaker ones; "empty": nothing at all; "weak": a few strong and many weaker.
        const items = q.includes("empty")
          ? []
          : q.includes("nothing")
            ? weaker ? searchHits(0, 6) : []
            : q.includes("weak")
              ? weaker ? searchHits(8, 12) : searchHits(8)
              : searchHits(q ? 24 : 30);
        const ranked = q !== "";
        const extra = !ranked
          ? {}
          : {
              strong_count: q.includes("nothing") ? 0 : items.length,
              weaker: !weaker && (q.includes("nothing") || q.includes("weak")) ? { count: q.includes("weak") ? 12 : 6, cursor: q.includes("weak") ? "after-strong" : null } : null,
              parsed: q.includes("summer") ? PARSED : { text: q, chips: [] },
              reindexing: processing === REINDEXING ? { model: "siglip2-b16", indexed: 20800, total: 52000, percent: 40, switch_at_percent: 95 } : null,
            };
        return route.fulfill(json({ items, next_cursor: null, ranked, total: ranked ? null : items.length, ...extra }));
      }
      // "More like this": the photo is IMG_1003 (a3) under a uuid; the one ending in `dead` is not indexed yet.
      const sm = /^\/assets\/([0-9a-f-]{36})\/similar$/.exec(p);
      if (sm && m === "GET") {
        if (sm[1]!.endsWith("dead"))
          return route.fulfill({ status: 409, contentType: "application/json", body: '{"error":"this photo is still being indexed: similar photos are available once it is"}' });
        const weaker = u.searchParams.get("weaker") === "true";
        const items = weaker ? searchHits(8, 12) : searchHits(8);
        return route.fulfill(
          json({ items, next_cursor: null, ranked: true, total: null, strong_count: 8, weaker: weaker ? null : { count: 12, cursor: "after-strong" }, similar_to: sm[1] }),
        );
      }
      const dtm = /^\/assets\/([^/]+)\/details$/.exec(p);
      if (dtm && m === "GET") return route.fulfill(json(Number(dtm[1]!.replace(/\D/g, "")) % 2 === 0 ? fullDetails(dtm[1]!) : bareDetails(dtm[1]!)));
      const um = /^\/assets\/([0-9a-f-]{36})$/.exec(p);
      if (um && m === "GET") return route.fulfill(json({ ...assets[3], id: um[1], albums: [], original_url: thumb(3), preview_url: thumb(3) }));
      if (p === "/assets/processing" && m === "GET") return route.fulfill(json(processing));
      if (p === "/assets/processing/retry" && m === "POST") {
        processing = { ...IDLE };
        return route.fulfill(json({ requeued: 1234 }));
      }
      if (p === "/assets/check") return route.fulfill({ status: 500, contentType: "application/json", body: '{"error":"fixture: upload refused"}' });
      if (p === "/assets/trash" && m === "GET")
        return route.fulfill(json({ items: trash, next_cursor: null, total: trash.length, retention_days: 30 }));
      if (p === "/assets/albums" && m === "GET") return route.fulfill(json({ items: albums }));
      const am = /^\/assets\/albums\/([^/]+)(\/assets)?$/.exec(p);
      if (am && m === "GET") {
        if (am[2]) return route.fulfill(json({ items: assets.slice(0, 18), next_cursor: null, next_before: null }));
        return route.fulfill(json(albums.find((a) => a.id === am[1]) ?? albums[0]));
      }
      if (p === "/assets/timeline" && m === "GET") return route.fulfill(json(timelineOf(assets as FakeAsset[], false)));
      if (p === "/assets" && m === "GET") return route.fulfill(json(pageOf(assets as FakeAsset[], u.searchParams)));
      const dm = /^\/assets\/(a\d+)$/.exec(p);
      if (dm && m === "GET") {
        const a = assets.find((x) => x.id === dm[1])!;
        return route.fulfill(
          json({
            ...a,
            albums: [
              { id: "l1", title: LONG_ALBUM },
              { id: "l2", title: "Pets" },
              { id: "l3", title: albums[2]!.title },
            ],
            original_url: thumb(0),
            preview_url: thumb(Number(a.id.slice(1))),
          }),
        );
      }
      if (p === "/assets/trash" && m === "POST")
        return route.fulfill(json({ count: 48213, batch: "2026-10-16T00:00:00Z" }));
      if (m === "POST" || m === "PATCH" || m === "DELETE") return route.fulfill(json({ count: 1 }));
      return route.fulfill({ status: 404, contentType: "application/json", body: '{"error":"not mocked"}' });
    },
  );
}

// Share links: the owner's list, create and edit calls, and the public viewer's. Registered after `mock`, so these win
// for the paths they handle and everything else falls through to it.
const TOKEN = (n: number) => `tok${n}`.padEnd(32, "x");
const share = (n: number, over: Record<string, unknown> = {}) => ({
  id: `s${n}`,
  kind: "selection",
  album: null,
  title: null,
  token: TOKEN(n),
  has_password: false,
  allow_download: true,
  include_originals: false,
  expires_at: null,
  expired: false,
  item_count: 12,
  view_count: 3,
  last_viewed_at: null,
  created_at: "2024-06-01T00:00:00Z",
  updated_at: "2024-06-01T00:00:00Z",
  ...over,
});
const shares = [
  share(1, { kind: "album", album: { id: "l1", title: LONG_ALBUM }, item_count: 30, has_password: true, expires_at: "2099-01-01T00:00:00Z", view_count: 1234 }),
  share(2, { title: "Averyveryveryveryveryveryveryveryveryveryveryverylongsinglewordsharetitle", include_originals: true, expired: true, expires_at: "2020-01-01T00:00:00Z" }),
  share(3, { allow_download: false }),
];
const publicItem = (n: number, downloads: unknown[]) => ({
  id: `p${n}`,
  width: 4000,
  height: 3000,
  sort_time: new Date(Date.UTC(2024, 5, 30 - n, 12)).toISOString(),
  thumbhash: null,
  thumb_url: thumb(n),
  preview_url: thumb(n),
  downloads,
  download_pending: false,
});
const PUBLIC_ITEMS = Array.from({ length: 18 }, (_, n) =>
  publicItem(
    n,
    n === 0
      ? [
          { id: "p0", label: "JPEG", size_bytes: 3_000_000 },
          { id: "raw0", label: "RAW", size_bytes: 25_000_000 },
        ]
      : [{ id: `p${n}`, label: "JPEG", size_bytes: null }],
  ),
);
let unlocked = false;

async function mockShares(page: Page) {
  const json = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(
    (u) => /^\/assets\/(shares|public\/s\/)/.test(u.pathname),
    async (route) => {
      const req = route.request();
      const p = new URL(req.url()).pathname;
      const m = req.method();
      if (p === "/assets/shares" && m === "GET") return route.fulfill(json({ items: shares }));
      if (p === "/assets/shares" && m === "POST") return route.fulfill(json(share(9, { title: LONG_ALBUM, item_count: 2 }), 201));
      if (/^\/assets\/shares\/s\d+$/.test(p)) return route.fulfill(m === "DELETE" ? { status: 204 } : json(shares[0]));
      const pm = /^\/assets\/public\/s\/([^/]+)(\/.*)?$/.exec(p);
      if (!pm) return route.fallback();
      const [, token, rest = ""] = pm;
      if (token === "gone") return route.fulfill(json({ error: "not found" }, 404));
      const locked = token === "locked" && !unlocked;
      if (rest === "") {
        if (locked) return route.fulfill(json({ needs_password: true, unlocked: false }));
        return route.fulfill(
          json({
            needs_password: token === "locked",
            unlocked: true,
            kind: "album",
            title: LONG_ALBUM,
            item_count: PUBLIC_ITEMS.length,
            allow_download: true,
            include_originals: true,
            expires_at: new Date(Date.now() + 3 * 86_400_000).toISOString(),
          }),
        );
      }
      if (rest === "/unlock") {
        const ok = (req.postDataJSON() as { password: string }).password === "right";
        if (ok) unlocked = true;
        return route.fulfill(ok ? { status: 204 } : json({ error: "unauthorized" }, 401));
      }
      if (rest === "/items") return route.fulfill(json({ items: PUBLIC_ITEMS, next_cursor: null }));
      return route.fulfill(json({ error: "not mocked" }, 404));
    },
  );
}

// The admin area: the invite list with every state and a long note, a long address and long limits, the link shown after
// creating one, the register page with an invite in the address bar (valid or not) and without one on an invite-only
// server, and what a member sees at /admin.
const LONG_EMAIL = "an.extraordinarily.long.address.for.a.guest@a-very-long-domain-name-for-photos.example.org";
const LONG_TOKEN = "0123456789abcdef".repeat(4);
const inv = (n: number, over: Record<string, unknown> = {}) => ({
  id: `i${n}`,
  note: null,
  status: "pending",
  created_at: "2024-06-01T00:00:00Z",
  created_by: "someone.with.a.long.address@example-photo-library.org",
  expires_at: new Date(Date.now() + 5 * 86_400_000).toISOString(),
  revoked_at: null,
  used_at: null,
  used_by: null,
  limits: { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null },
  ...over,
});
const INVITES = [
  inv(1, { note: "For Sam, " + "who_has_a_really_long_unbroken_note_".repeat(3), limits: { quota_bytes: 5_497_558_138_880, daily_upload_bytes: 1_073_741_824, max_file_bytes: 214_748_364_800 } }),
  inv(2, { status: "used", used_at: "2024-06-02T00:00:00Z", used_by: LONG_EMAIL, note: "Used" }),
  inv(3, { status: "expired", note: "Expired", expires_at: "2024-06-05T00:00:00Z" }),
  inv(4, { status: "revoked", note: "Revoked", revoked_at: "2024-06-03T00:00:00Z" }),
];
let inviteCheck: { valid: boolean; reason?: string } = { valid: true };
const registration = { mode: "invite", first_account: false, open: false, invite_required: true };

async function mockAdmin(page: Page) {
  const json = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(
    (u) => /^\/auth\/(admin|invites|registration)/.test(u.pathname),
    async (route) => {
      const p = new URL(route.request().url()).pathname;
      const m = route.request().method();
      if (p === "/auth/registration") return route.fulfill(json(registration));
      if (p === "/auth/invites/check") return route.fulfill(json(inviteCheck));
      if (p === "/auth/admin/invites" && m === "GET") return route.fulfill(json({ items: INVITES }));
      if (p === "/auth/admin/invites" && m === "POST")
        return route.fulfill(json({ invite: INVITES[0], token: LONG_TOKEN, path: `/register?invite=${LONG_TOKEN}` }, 201));
      if (/reissue$/.test(p)) return route.fulfill(json({ invite: INVITES[0], token: LONG_TOKEN, path: `/register?invite=${LONG_TOKEN}` }));
      return route.fulfill(json({}, 204));
    },
  );
}

/** Assert that nothing sticks out of the viewport (and, with SHOT_DIR, save a screenshot named `<prefix>-<name>.png`). */
function makeCheck(page: Page, prefix: string, problems: string[]) {
  return async function check(name: string) {
    await page.waitForTimeout(150);
    const r = await page.evaluate(() => {
      const w = window.innerWidth;
      const bad: string[] = [];
      for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
        const b = el.getBoundingClientRect();
        // Tiles clip their hover-zoomed image; map markers sit wherever the map puts them and the pane clips them.
        if (b.width === 0 || el.closest(".tile") || el.closest(".map-canvas")) continue;
        if (b.right > w + 0.5 || b.left < -0.5) {
          bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} "${(el.textContent ?? "").trim().slice(0, 24)}" [${Math.round(b.left)}..${Math.round(b.right)}]`);
        }
      }
      // The page can also be pushed wide by something whose own box fits but whose content spills (scrollWidth): name those.
      if (document.documentElement.scrollWidth > w) {
        for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
          if (el.scrollWidth > w + 0.5 && !el.closest(".tile") && !el.closest(".map-canvas")) {
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

for (const { width, scheme } of RUNS) {
  test.describe(`${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: { width, height: width === 1280 ? 800 : 740 }, hasTouch: width < 600 });

    test(`no horizontal overflow on any page or overlay`, async ({ page }) => {
      test.setTimeout(90_000);
      if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
      const problems: string[] = [];

      const check = makeCheck(page, `${width}${scheme === "dark" ? "-dark" : ""}`, problems);
      const open = async (path: string, ready: string | RegExp) => {
        await page.goto(path);
        await (typeof ready === "string" ? page.getByText(ready) : page.getByRole("button", { name: ready })).first().waitFor();
        await page.waitForLoadState("networkidle");
      };

      await mock(page);
      await mockShares(page);
      unlocked = false;

      // Auth
      await page.route("**/auth/me", (r) => r.fulfill({ status: 401, contentType: "application/json", body: '{"error":"no"}' }));
      await open("/login", "Sign in");
      await check("login");
      await open("/register", "Create");
      await check("register");
      await page.unroute("**/auth/me");

      // Library
      await open("/library", /Open IMG_1000/);
      await check("library");
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_1000/ }).click();
      await page.getByRole("button", { name: /Select IMG_1001/ }).click();
      await check("library-select");
      await page.getByRole("button", { name: "Select all" }).click();
      await check("library-select-all");
      // Edit the selection: choose what, then a shift of the dates, one location, one caption
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await page.getByRole("dialog", { name: "Edit photos" }).waitFor();
      await check("dialog-edit-menu");
      await page.getByRole("button", { name: "Adjust date and time" }).click();
      await page.getByRole("dialog", { name: /Adjust date and time, \d+ photos/ }).waitFor();
      await check("dialog-edit-shift");
      await page.keyboard.press("Escape");
      await page.getByRole("dialog").waitFor({ state: "detached" });
      await page.getByRole("button", { name: "Add to album" }).click();
      await page.getByRole("dialog").waitFor();
      await check("dialog-add-to-album");
      await page.getByRole("button", { name: /new album/i }).click().catch(() => undefined);
      await check("dialog-add-to-album-new");
      await page.keyboard.press("Escape");
      // Select all means the whole library, so trashing it asks first and names the exact count.
      await page.getByRole("button", { name: "Move to trash" }).last().click();
      await page.getByRole("alertdialog").getByRole("heading", { name: /^Move [\d,]+ photos to trash\?$/ }).waitFor();
      await check("dialog-confirm-trash-all");
      await page.getByRole("alertdialog").getByRole("button", { name: "Move to trash" }).click();
      await page.getByText(/[\d,]+ moved to trash/).waitFor();
      await check("library-toast-undo");

      // Processing indicator: banner while photos are processed, the failures panel, and after a retry
      processing = BUSY;
      await open("/library", /Open IMG_1000/);
      await page.getByText("Processing 20,617 photos…").waitFor();
      await check("library-processing");
      await page.getByRole("button", { name: "1,234 failed" }).click();
      await page.getByRole("button", { name: "Retry failed" }).waitFor();
      await check("library-processing-panel");
      await page.getByRole("button", { name: "Retry failed" }).click();
      await page.getByText("Processing 20,617 photos…").waitFor({ state: "detached", timeout: 10_000 });
      await check("library-processing-done");
      processing = IDLE;

      // Map: with a basemap, a photo from a marker in the lightbox, and the plain-background fallback without one
      const mapReady = () =>
        page.locator(".map-thumb, .map-bubble, .map-notice[role=alert]").first().waitFor({ timeout: 20_000 });
      basemapInstalled = true;
      await open("/map", "Photos in view");
      await mapReady();
      await page.waitForTimeout(1500); // tiles and labels draw
      await check("map");
      // A thumbnail that is inside the pane and not under another marker (they overlap at world zoom).
      let shown = null;
      for (const t of await page.locator(".map-thumb").all()) {
        const onTop = await t.evaluate((el) => {
          const r = el.getBoundingClientRect();
          const pane = document.querySelector(".map-pane")!.getBoundingClientRect();
          const inside = r.left > pane.left && r.right < pane.right && r.top > pane.top && r.bottom < pane.bottom;
          return inside && document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) === el;
        });
        if (onTop) {
          shown = t;
          break;
        }
      }
      if (shown) {
        await shown.click();
        await page.getByRole("dialog").waitFor();
        await check("map-lightbox");
        await page.keyboard.press("Escape");
      }
      await page.getByRole("button", { name: /Open IMG_1000/ }).click();
      await page.getByRole("dialog").waitFor();
      await check("map-list-lightbox");
      await page.keyboard.press("Escape");
      basemapInstalled = false;
      await open("/map", "Photos in view");
      await page.getByText(/No basemap is installed/).waitFor();
      await mapReady();
      await check("map-no-basemap");
      basemapInstalled = true;

      // Lightbox
      await open("/library", /Open IMG_1000/);
      await page.getByRole("button", { name: /Open IMG_20240615/ }).click();
      await page.getByText("In albums:").waitFor();
      await check("lightbox-long-name");
      await page.getByRole("button", { name: "Next photo" }).click();
      await check("lightbox");

      // Photo details: the Info button (then `i` and Escape), a photo with every row, then one with none but the file
      await page.getByRole("button", { name: "Info" }).click();
      const panel = page.getByRole("complementary", { name: "Photo details" });
      await panel.waitFor();
      await panel.getByText("Time zone unknown").waitFor();
      await check("lightbox-info-bare");
      await page.getByRole("button", { name: "Previous photo" }).click();
      await panel.getByText("Altitude 38 m above sea level").waitFor();
      await page.locator(".place-map canvas").first().waitFor({ timeout: 20_000 });
      await page.waitForTimeout(1200); // basemap tiles draw
      await check("lightbox-info");
      // The editors: location (map, your places, coordinates), date and time, caption
      await panel.getByRole("button", { name: "Edit location" }).click();
      await page.getByRole("dialog", { name: "Set location" }).waitFor();
      await page.getByRole("listitem").first().waitFor();
      await page.waitForTimeout(800);
      await check("edit-location");
      await page.keyboard.press("Escape");
      await panel.getByRole("button", { name: "Adjust date and time" }).click();
      await page.getByRole("dialog", { name: "Adjust date and time" }).waitFor();
      await check("edit-date");
      await page.keyboard.press("Escape");
      await panel.getByRole("button", { name: "Edit caption" }).click();
      await page.getByRole("dialog", { name: "Edit caption" }).waitFor();
      await check("edit-caption");
      await page.keyboard.press("Escape");
      await page.getByRole("dialog", { name: "Edit caption" }).waitFor({ state: "detached" });
      // The panel's own scroll reaches its last row, and the sheet never grows past the screen.
      await panel.evaluate((el) => el.scrollTo(0, el.scrollHeight));
      await panel.getByText("3 of 12 in a burst").waitFor();
      await check("lightbox-info-scrolled");
      await page.keyboard.press("Escape");
      await panel.waitFor({ state: "detached" });
      await page.getByRole("dialog").waitFor();
      await page.keyboard.press("i");
      await panel.waitFor();
      await page.keyboard.press("i");
      await panel.waitFor({ state: "detached" });
      await check("lightbox-info-closed");

      // Upload tray (the mocked check endpoint refuses, so every file shows as failed)
      await open("/library", /Open IMG_1000/);
      await page.locator('input[aria-label="Choose photos"]').setInputFiles([
        { name: LONG_NAME, mimeType: "image/jpeg", buffer: Buffer.from("x") },
        { name: "second.jpg", mimeType: "image/jpeg", buffer: Buffer.from("y") },
      ]);
      await page.getByRole("region", { name: "Uploads" }).waitFor();
      await page.getByText(/Failed/).first().waitFor();
      await check("upload-tray");
      await page.getByRole("button", { name: "Hide" }).click();
      await check("upload-tray-collapsed");

      // Trash
      await open("/trash", "Trash");
      await page.getByRole("button", { name: /Select IMG_1000/ }).waitFor();
      await check("trash");
      await page.getByRole("button", { name: /Select IMG_1001/ }).click();
      await check("trash-select");
      await page.getByRole("button", { name: "Delete forever" }).click();
      await check("dialog-delete-forever");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "Empty trash" }).click();
      await check("dialog-empty-trash");
      await page.getByRole("button", { name: "Cancel" }).click();

      // Albums
      await open("/albums", "Albums");
      await page.getByRole("link", { name: /Pets/ }).waitFor();
      await check("albums");
      await page.getByRole("button", { name: "New album" }).click();
      await check("dialog-new-album");
      await page.keyboard.press("Escape");

      // Album page
      await open("/albums/l1", "Edit");
      await page.getByRole("button", { name: /Open IMG_1000/ }).waitFor();
      await check("album");
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await check("album-edit");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_1001/ }).click();
      await check("album-select");
      await page.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: "Delete album" }).click();
      await check("dialog-delete-album");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "Add photos" }).click();
      await page.getByRole("button", { name: /Select IMG_1000/ }).waitFor();
      await page.getByRole("button", { name: /Select IMG_1003/ }).click();
      await check("album-photo-picker");
      await page.getByRole("dialog", { name: "Add photos" }).getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: /Open IMG_1000/ }).click();
      await check("album-lightbox");

      // Share links: the dialog (with and without the originals warning, and once created), the owner's list with its
      // edit and revoke dialogs, and the public viewer (grid, viewer with a RAW + JPEG download menu, password form, gone).
      await open("/library", /Open IMG_1000/);
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_1000/ }).click();
      await page.getByRole("button", { name: "Share", exact: true }).click();
      await page.getByRole("dialog", { name: /Share 1 photo/ }).waitFor();
      await check("dialog-share");
      await page.getByLabel(/Include originals/).check();
      await page.getByLabel("Password (optional)").fill("a-fairly-long-password-to-type");
      await check("dialog-share-originals");
      await page.getByRole("button", { name: "Create link" }).click();
      await page.getByRole("heading", { name: "Link created" }).waitFor();
      await check("dialog-share-created");
      await page.getByRole("dialog").getByRole("button", { name: "Done" }).click();

      await open("/shared", "Shared links");
      await page.getByRole("list", { name: "Shared links" }).waitFor();
      await check("shared-links");
      await page.getByRole("button", { name: "Edit" }).first().click();
      await page.getByRole("dialog", { name: "Edit link" }).waitFor();
      await check("dialog-share-edit");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "Revoke" }).first().click();
      await check("dialog-share-revoke");
      await page.getByRole("button", { name: "Cancel" }).click();

      await open("/s/open", /Open photo 1 of/);
      await check("share-view");
      await page.getByRole("button", { name: /Open photo 1 of/ }).click();
      await page.getByRole("dialog", { name: "Photo" }).getByRole("button", { name: "Download" }).click();
      await page.getByRole("link", { name: /Download RAW/ }).waitFor();
      await check("share-view-lightbox");
      await page.keyboard.press("Escape");
      await open("/s/locked", "This link is protected");
      await check("share-password");
      await page.getByLabel("Password").fill("wrong");
      await page.getByRole("button", { name: "View photos" }).click();
      await page.getByText("That password is not right.").waitFor();
      await check("share-password-error");
      await page.getByLabel("Password").fill("right");
      await page.getByRole("button", { name: "View photos" }).click();
      await page.getByRole("button", { name: /Open photo 1 of/ }).waitFor();
      await check("share-view-unlocked");
      await open("/s/gone", "This link is not available");
      await check("share-gone");

      // Timeline of a larger library: month and day headers, the scrubber (resting and dragged), hearts, downloads.
      await installLibrary(page, makeAssets(4000), (r) => r.fallback());
      await open("/library", /Open IMG_00000/);
      await page.locator(".scrubber").waitFor();
      await check("timeline");
      const handle = (await page.getByRole("slider").boundingBox())!;
      const rail = (await page.locator(".scrubber").boundingBox())!;
      await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
      await page.mouse.down();
      await page.mouse.move(handle.x + handle.width / 2, rail.y + rail.height * 0.55, { steps: 6 });
      await page.locator(".scrubber-bubble").waitFor();
      await check("timeline-scrubber-drag");
      await page.mouse.up();
      await page.locator(".tile-cell img").first().waitFor();
      await check("timeline-scrolled");
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_00001/ }).click();
      await check("timeline-select");
      await page.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: /Open IMG_00000/ }).click(); // a RAW + JPEG pair
      await page.getByRole("dialog").getByRole("button", { name: "Download" }).click();
      await page.getByRole("link", { name: /Download RAW/ }).waitFor();
      await check("lightbox-download-pair");
      await page.getByRole("button", { name: "Add to favorites" }).click();
      await check("lightbox-favorited");
      await page.keyboard.press("Escape");
      await open("/favorites", /Open IMG_00003/);
      await check("favorites-view");

      // API keys
      await open("/settings/api-keys", "API keys");
      await page.getByText("nightly import").waitFor();
      await page.getByLabel("Name").fill("new");
      await page.getByRole("button", { name: "Create key" }).click();
      await page.getByText("Copy your new key").waitFor();
      await check("api-keys");

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test(`search: bar, results, filters, chips and states fit`, async ({ page }) => {
      test.setTimeout(90_000);
      if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
      const problems: string[] = [];
      const check = makeCheck(page, `${width}${scheme === "dark" ? "-dark" : ""}-search`, problems);
      await mock(page);
      processing = IDLE;
      const field = page.getByRole("combobox", { name: "Search photos" });
      // A phone shows an icon that expands; a wide screen shows the field. Either way this leaves the field usable.
      const openBar = async () => {
        const icon = page.getByRole("button", { name: "Search photos" });
        if (await icon.isVisible()) await icon.click();
        await field.waitFor({ state: "visible" });
      };

      await page.goto("/library");
      await page.getByRole("button", { name: /Open IMG_1000/ }).first().waitFor();
      await check("bar-collapsed");
      expect(await page.getByRole("link").filter({ hasText: /^(All photos|Favorites|Map)$/ }).count()).toBe(3);
      await openBar();
      await check("bar-expanded");
      // Recent searches drop down under the field.
      await page.evaluate(() => localStorage.setItem("cyanotype.recentSearches", JSON.stringify(["dog on the beach", "red car at night", "x".repeat(90)])));
      await field.fill("");
      await field.blur(); // (a phone's field is already focused after expanding)
      await field.focus();
      await page.getByRole("listbox", { name: "Recent searches" }).waitFor();
      await check("bar-recent");

      await field.fill("dog on the beach");
      await field.press("Enter");
      await page.getByRole("button", { name: /Open IMG_1000/ }).first().waitFor();
      await page.getByText(/^\d+ matches$/).waitFor();
      await check("results");
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_1000/ }).click();
      await check("results-select");
      await page.getByRole("button", { name: "Done" }).click();

      await page.getByRole("button", { name: /^Filters/ }).click();
      await page.getByRole("dialog", { name: "Filters" }).waitFor();
      await page.getByRole("option", { name: new RegExp(LONG_CAMERA.slice(0, 20)) }).first().waitFor({ state: "attached" });
      await check("filters-sheet");
      const sheet = page.getByRole("dialog", { name: "Filters" });
      await sheet.getByLabel("Camera", { exact: true }).selectOption(LONG_CAMERA);
      await sheet.getByLabel("Place", { exact: true }).selectOption(LONG_PLACE);
      await sheet.getByLabel("Year", { exact: true }).selectOption("2024");
      await sheet.getByRole("checkbox", { name: /Favorites only/ }).check();
      await sheet.getByRole("radio", { name: "RAW" }).check({ force: true });
      await check("filters-sheet-filled");
      await page.getByRole("button", { name: "Show photos" }).click();
      await page.getByRole("list", { name: "Active filters" }).getByText("2024").waitFor();
      await check("results-with-chips");

      await openBar();
      await field.fill("empty like it");
      await field.press("Enter");
      await page.getByRole("heading", { name: "Nothing matched" }).waitFor();
      await check("no-results");

      // The query read as a date and a place: two chips (the place as long as a place name can be), and taking one off.
      await page.goto("/search?q=dog%20in%20summer%202022");
      await page.getByRole("list", { name: "Understood from your search" }).waitFor();
      await check("parsed-chips");
      await page.getByRole("button", { name: "Remove date Summer 2022" }).click();
      await page.waitForURL(/skip=date/);
      await check("parsed-chip-removed");

      // Strong matches with weaker ones behind them, and the weaker ones shown.
      await page.goto("/search?q=weak");
      await page.getByRole("button", { name: /Show weaker matches \(12\)/ }).waitFor();
      await check("weaker-offered");
      await page.getByRole("button", { name: /Show weaker matches/ }).click();
      await page.getByText("8 matches, 12 weaker").waitFor();
      await check("weaker-shown");
      // Nothing close, but weaker matches exist; and nothing at all.
      await page.goto("/search?q=nothing%20close");
      await page.getByRole("heading", { name: "No close matches" }).waitFor();
      await check("no-close-matches");
      await page.getByRole("button", { name: /Show weaker matches \(6\)/ }).click();
      await page.getByText("0 matches, 6 weaker").waitFor();
      await check("no-close-matches-weaker-shown");
      await page.goto("/search?q=empty");
      await page.getByRole("heading", { name: "Nothing matched" }).waitFor();
      await check("nothing-at-all");

      // An older model answers while the improved one indexes the library: said once, with progress.
      processing = REINDEXING;
      await page.goto("/search?q=dog");
      await page.getByRole("button", { name: /Open IMG_1000/ }).first().waitFor();
      await page.getByText(/Re-indexing with the improved search model: 40%/).first().waitFor();
      await check("reindexing");
      await page.goto("/search");
      await page.getByRole("heading", { name: "Search your photos" }).waitFor();
      await page.getByText(/Re-indexing with the improved search model: 40%/).waitFor();
      await check("reindexing-intro");

      processing = INDEXING;
      await page.goto("/search?q=dog");
      await page.getByRole("button", { name: /Open IMG_1000/ }).first().waitFor();
      await page.getByText(/Still indexing 39,655 photos/).first().waitFor();
      await check("indexing");
      await page.goto("/search?q=nothing");
      await page.getByText(/still being indexed cannot match/).waitFor();
      await check("indexing-no-results");
      processing = IDLE;

      await page.goto("/search?q=unavailable");
      await page.getByRole("heading", { name: "Search isn’t available" }).waitFor();
      await check("unavailable");

      await page.goto("/search");
      await page.getByRole("heading", { name: "Search your photos" }).waitFor();
      await check("intro");

      // Bursts: collapsed with their badges, one opened inline, and selecting a collapsed burst (the bar says what it includes).
      await page.goto("/search?q=burst");
      await page.getByRole("button", { name: "Show 3 more photos from this burst" }).waitFor();
      await check("bursts");
      await page.getByRole("button", { name: "Show 3 more photos from this burst" }).click();
      await page.getByRole("button", { name: "Hide the other 3 photos of this burst" }).waitFor();
      await check("burst-open");
      await page.getByRole("button", { name: "Hide the other 3 photos of this burst" }).click();
      await page.getByRole("button", { name: "Select", exact: true }).click();
      await page.getByRole("button", { name: /Select IMG_1001.* and 3 more from its burst/ }).click();
      await page.getByText("4 selected").waitFor();
      await check("burst-selected");
      await page.getByRole("button", { name: "Done" }).click();

      // "More like this": the source chip with its thumbnail, the filters beside it, weaker matches, the lightbox button
      // that starts it, and a photo that is not indexed yet.
      await page.goto("/search?similar=6f1c2b0e-0000-4000-8000-0000000000a3&favorite=true");
      await page.getByText("8 similar photos").waitFor();
      await check("similar");
      await page.getByRole("button", { name: /Show weaker matches \(12\)/ }).click();
      await page.getByText("8 similar photos, 12 weaker").waitFor();
      await check("similar-weaker-shown");
      await page.getByRole("button", { name: /Open IMG_1000/ }).first().click();
      await page.getByRole("button", { name: "Similar photos" }).waitFor();
      await check("similar-lightbox");
      await page.keyboard.press("Escape");
      await page.goto("/search?similar=6f1c2b0e-0000-4000-8000-00000000dead");
      await page.getByRole("heading", { name: "This photo is still being indexed" }).waitFor();
      await check("similar-not-indexed");

      expect(problems, problems.join("\n")).toEqual([]);
    });

    test(`admin: invites and the invite-only register page fit`, async ({ page }) => {
      test.setTimeout(90_000);
      if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
      const problems: string[] = [];
      const check = makeCheck(page, `${width}${scheme === "dark" ? "-dark" : ""}-admin`, problems);
      await mock(page);
      await mockAdmin(page);
      memberView = false;
      await page.goto("/admin");
      await page.getByRole("heading", { name: "Invites" }).waitFor();
      await page.getByText("Used by " + LONG_EMAIL).waitFor();
      await check("invites");
      await page.getByText("Starting limits (optional)").click();
      await page.getByLabel(/Note/).fill("For Sam, " + "who_has_a_really_long_unbroken_note_".repeat(3));
      await page.getByLabel("Storage (GB)").fill("5120.5");
      await check("invites-form");
      await page.getByRole("button", { name: "Create invite" }).click();
      await page.getByText("Copy this link now").waitFor();
      await check("invites-new-link");

      anonymous = true;
      inviteCheck = { valid: true };
      await page.goto(`/register?invite=${LONG_TOKEN}`);
      await page.getByText("You have been invited").waitFor();
      await check("register-invited");
      inviteCheck = { valid: false, reason: "expired" };
      await page.goto(`/register?invite=${LONG_TOKEN}`);
      await page.getByText(/has expired/).waitFor();
      await check("register-invite-expired");
      await page.goto("/register");
      await page.getByLabel("Invite link or code").waitFor();
      await check("register-needs-invite");
      anonymous = false;

      memberView = true;
      await page.goto("/admin");
      await page.getByRole("heading", { name: "Page not found" }).waitFor();
      await check("admin-member-404");
      memberView = false;

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
