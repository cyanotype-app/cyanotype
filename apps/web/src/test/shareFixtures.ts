import type { PublicInfo, PublicItem, ShareLink } from "../shareApi";

export const link = (n: number, over: Partial<ShareLink> = {}): ShareLink => ({
  id: `share-${n}`,
  kind: "selection",
  album: null,
  title: null,
  token: `tok${n}`.padEnd(32, "x"),
  has_password: false,
  allow_download: true,
  include_originals: false,
  expires_at: null,
  expired: false,
  item_count: 3,
  view_count: 0,
  last_viewed_at: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  ...over,
});

export const pubItem = (n: number, over: Partial<PublicItem> = {}): PublicItem => ({
  id: `p-${n}`,
  width: 400,
  height: 300,
  sort_time: new Date(Date.UTC(2026, 0, 30 - n)).toISOString(),
  thumbhash: null,
  thumb_url: `https://s3.test/thumb-${n}`,
  preview_url: `https://s3.test/preview-${n}`,
  downloads: [{ id: `p-${n}`, label: "JPEG", size_bytes: null }],
  download_pending: false,
  ...over,
});

export const pubInfo = (over: Partial<PublicInfo> = {}): PublicInfo => ({
  needs_password: false,
  unlocked: true,
  kind: "selection",
  title: "Summer trip",
  item_count: 3,
  allow_download: true,
  include_originals: false,
  expires_at: null,
  ...over,
});
