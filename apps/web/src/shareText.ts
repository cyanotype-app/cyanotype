import { photoCount } from "./albumText";
import type { ShareLink } from "./shareApi";

export type ExpiryPreset = "1d" | "1w" | "1m" | "never";

export const EXPIRY_PRESETS: { id: ExpiryPreset; label: string; days: number | null }[] = [
  { id: "1d", label: "1 day", days: 1 },
  { id: "1w", label: "1 week", days: 7 },
  { id: "1m", label: "1 month", days: 30 },
  { id: "never", label: "Never", days: null },
];

/** The RFC 3339 expiry for a preset counted from `now`; null for "never". */
export function expiryFromPreset(id: ExpiryPreset, now = new Date()): string | null {
  const days = EXPIRY_PRESETS.find((p) => p.id === id)?.days ?? null;
  return days === null ? null : new Date(now.getTime() + days * 86_400_000).toISOString();
}

/** The address to hand out. */
export const shareUrl = (token: string, origin = window.location.origin) => `${origin}/s/${token}`;

const DAY = 86_400_000;

/** "Never expires", "Expires in 3 days", "Expires today", "Expired 2 days ago". */
export function expiryText(expiresAt: string | null, now = new Date()): string {
  if (!expiresAt) return "Never expires";
  const t = new Date(expiresAt).getTime();
  if (Number.isNaN(t)) return "Never expires";
  const diff = t - now.getTime();
  if (diff <= 0) {
    const days = Math.floor(-diff / DAY);
    return days === 0 ? "Expired today" : `Expired ${days} day${days === 1 ? "" : "s"} ago`;
  }
  const hours = Math.ceil(diff / 3_600_000);
  if (hours < 24) return hours <= 1 ? "Expires within the hour" : `Expires in ${hours} hours`;
  const days = Math.round(diff / DAY);
  return `Expires in ${days} day${days === 1 ? "" : "s"}`;
}

/** What to call a link in lists: its title, else the album's, else what it holds. */
export const shareName = (s: Pick<ShareLink, "title" | "album" | "item_count">) =>
  s.title ?? s.album?.title ?? photoCount(s.item_count);

/** One line of facts about a link. */
export function shareSummary(s: ShareLink, now = new Date()): string {
  const parts = [
    s.kind === "album" ? "Album" : "Selection",
    photoCount(s.item_count),
    expiryText(s.expires_at, now),
    `${s.view_count} view${s.view_count === 1 ? "" : "s"}`,
  ];
  return parts.join(" · ");
}

/** What visitors can download, in words. */
export function downloadText(s: Pick<ShareLink, "allow_download" | "include_originals">): string {
  if (!s.allow_download) return "No downloads";
  return s.include_originals ? "Downloads: originals, with location" : "Downloads: JPEG without location";
}

/** Copy to the clipboard; false when the browser refuses (no permission, insecure context). */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
