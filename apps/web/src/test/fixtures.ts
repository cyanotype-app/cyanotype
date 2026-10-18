import type { Album, AssetDetail, AssetListItem, PhotoDetails, TrashItem } from "../api";

export const item = (n: number, over: Partial<AssetListItem> = {}): AssetListItem => ({
  id: `id-${n}`,
  original_name: `photo-${n}.jpg`,
  content_type: "image/jpeg",
  size_bytes: 1000 + n,
  width: 400,
  height: 300,
  sort_time: new Date(Date.UTC(2026, 0, 30 - n)).toISOString(),
  thumbhash: null,
  thumb_url: `https://s3.test/thumb-${n}`,
  ...over,
});

export const detail = (n: number): AssetDetail => ({
  ...item(n),
  original_url: `https://s3.test/original-${n}`,
  preview_url: `https://s3.test/preview-${n}`,
});

export const trashed = (n: number, over: Partial<TrashItem> = {}): TrashItem => ({
  ...item(n),
  deleted_at: new Date(Date.now() - 3 * 86_400_000).toISOString(),
  purge_at: new Date(Date.now() + 27 * 86_400_000 + 3_600_000).toISOString(),
  ...over,
});

export const album = (n: number, over: Partial<Album> = {}): Album => ({
  id: `album-${n}`,
  title: `Album ${n}`,
  description: null,
  photo_count: n,
  cover: { id: `id-${n}`, thumbhash: null, thumb_url: `https://s3.test/thumb-${n}` },
  first_at: "2025-06-10T12:00:00Z",
  last_at: "2026-01-20T12:00:00Z",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-02T00:00:00Z",
  ...over,
});

/** What GET /assets/{id}/details answers for a fully described photo (a HEIC taken in Lisbon, UTC+2). */
export const photoDetails = (over: Partial<PhotoDetails> = {}): PhotoDetails => ({
  id: "id-1",
  date: {
    instant: "2023-07-04T11:05:09Z",
    local: "2023-07-04T13:05:09",
    offset: "+02:00",
    zone_known: true,
    weekday: "Tuesday",
    basis: "taken",
    timeline_day: "2023-07-04",
  },
  location: {
    latitude: 38.7223,
    longitude: -9.1393,
    place: { city: "Lisbon", region: "Lisbon", country: "Portugal", country_code: "PT" },
    altitude: "38 m above sea level",
    altitude_m: 37.8,
    direction: "132° SE",
    direction_deg: 132.4,
  },
  camera: { label: "Apple iPhone 15", make: "Apple", model: "iPhone 15", lens: "iPhone 15 back camera 6.765mm f/1.78" },
  exposure: {
    line: "ISO 100 · 6.8 mm · ƒ/1.8 · 1/120 s · +0.3 ev",
    iso: "ISO 100",
    focal_length: "6.8 mm",
    focal_length_35mm: "24 mm",
    aperture: "ƒ/1.8",
    shutter: "1/120 s",
    bias: "+0.3 ev",
    flash: "Did not fire",
    white_balance: "Auto",
    metering: "Pattern",
    program: "Normal program",
  },
  file: {
    name: "IMG_0001.HEIC",
    format: "HEIC",
    raw: false,
    content_type: "image/heic",
    size_bytes: 2_500_000,
    width: 4032,
    height: 3024,
    megapixels: "12 MP",
    color_space: "Uncalibrated",
    software: "17.4.1",
  },
  files: [{ id: "id-1", name: "IMG_0001.HEIC", format: "HEIC", raw: false, size_bytes: 2_500_000 }],
  albums: [{ id: "album-1", title: "Portugal" }],
  burst: { position: 1, size: 8 },
  ...over,
});

/** A photo with nothing but a file: every optional part absent. */
export const bareDetails = (): PhotoDetails =>
  photoDetails({
    date: { instant: "2024-02-03T04:05:06Z", local: "2024-02-03T04:05:06", offset: null, zone_known: false, weekday: "Saturday", basis: "added", timeline_day: "2024-02-03" },
    location: null,
    camera: null,
    exposure: null,
    file: { name: "scan.png", format: "PNG", raw: false, content_type: "image/png", size_bytes: 1000, width: null, height: null, megapixels: null, color_space: null, software: null },
    files: [{ id: "id-1", name: "scan.png", format: "PNG", raw: false, size_bytes: 1000 }],
    albums: [],
    burst: null,
  });

/** A photo with a RAW stacked behind it (the API's `paired` list). */
export const withRaw = (n: number, over: Partial<AssetListItem> = {}): AssetListItem =>
  item(n, {
    paired: [
      { id: `raw-${n}`, original_name: `photo-${n}.CR3`, content_type: "image/x-canon-cr3", size_bytes: 25_000_000, kind: "raw" },
    ],
    ...over,
  });
