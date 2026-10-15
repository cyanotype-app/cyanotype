import type { AssetDetail, AssetListItem, TrashItem } from "../api";

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
