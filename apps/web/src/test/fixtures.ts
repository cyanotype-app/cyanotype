import type { Album, AssetDetail, AssetListItem, TrashItem } from "../api";

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

/** A photo with a RAW stacked behind it (the API's `paired` list). */
export const withRaw = (n: number, over: Partial<AssetListItem> = {}): AssetListItem =>
  item(n, {
    paired: [
      { id: `raw-${n}`, original_name: `photo-${n}.CR3`, content_type: "image/x-canon-cr3", size_bytes: 25_000_000, kind: "raw" },
    ],
    ...over,
  });
