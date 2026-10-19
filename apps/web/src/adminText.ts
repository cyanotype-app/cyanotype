import type { Invite } from "./api";

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** "1.5 GB" (1024-based, like the rest of the app). */
export function bytesLabel(bytes: number): string {
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < UNITS.length - 1) {
    v /= 1024;
    u++;
  }
  return `${u === 0 || v >= 10 ? Math.round(v) : v.toFixed(1)} ${UNITS[u]}`;
}

/** A gigabyte amount typed into a form ("5", "0.5") as bytes; null for empty, undefined when it is not a positive number. */
export function gigabytesToBytes(text: string): number | null | undefined {
  const t = text.trim().replace(",", ".");
  if (t === "") return null;
  if (!/^\d+(\.\d+)?$/.test(t)) return undefined;
  const n = Number(t);
  if (!(n > 0)) return undefined;
  return Math.round(n * 1024 ** 3);
}

/** The absolute link of an invite path, for the clipboard. */
export const inviteUrl = (origin: string, path: string): string => `${origin}${path}`;

const DAY = 86_400_000;

/** "in 6 days", "in 5 hours", "in 12 minutes", or "expired" for a time that has passed. */
export function expiresIn(iso: string, now: number = Date.now()): string {
  const ms = new Date(iso).getTime() - now;
  if (ms <= 0) return "expired";
  if (ms >= DAY) {
    const d = Math.round(ms / DAY);
    return `in ${d} ${d === 1 ? "day" : "days"}`;
  }
  const h = Math.floor(ms / 3_600_000);
  if (h >= 1) return `in ${h} ${h === 1 ? "hour" : "hours"}`;
  const m = Math.max(1, Math.floor(ms / 60_000));
  return `in ${m} ${m === 1 ? "minute" : "minutes"}`;
}

/** One line for an invite's state: "Waiting, expires in 6 days", "Used by a@b.co", "Revoked", "Expired". */
export function inviteStatusLine(inv: Invite, now: number = Date.now()): string {
  switch (inv.status) {
    case "pending":
      return `Waiting, expires ${expiresIn(inv.expires_at, now)}`;
    case "used":
      return inv.used_by ? `Used by ${inv.used_by}` : "Used";
    case "revoked":
      return "Revoked";
    case "expired":
      return "Expired";
  }
}

/** The presets of an invite as words ("5 GB storage · 1 GB a day · files up to 200 MB"), or null when it has none. */
export function limitsLine(l: Invite["limits"]): string | null {
  const parts: string[] = [];
  if (l.quota_bytes) parts.push(`${bytesLabel(l.quota_bytes)} storage`);
  if (l.daily_upload_bytes) parts.push(`${bytesLabel(l.daily_upload_bytes)} a day`);
  if (l.max_file_bytes) parts.push(`files up to ${bytesLabel(l.max_file_bytes)}`);
  return parts.length ? parts.join(" · ") : null;
}
