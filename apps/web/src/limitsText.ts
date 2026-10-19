import type { Limits, MyUsage } from "./api";
import { bytesLabel } from "./adminText";

/** Where an account stands against one limit: fine, past 80 % (warn), past 95 % (critical) or at the limit (full). */
export type Level = "ok" | "warn" | "critical" | "full";

export function percentOf(used: number, limit: number): number {
  return limit > 0 ? Math.min(100, Math.floor((used / limit) * 100)) : 0;
}

export function levelOf(used: number, limit: number): Level {
  if (limit <= 0) return "ok";
  if (used >= limit) return "full";
  const pct = (used / limit) * 100;
  if (pct >= 95) return "critical";
  if (pct >= 80) return "warn";
  return "ok";
}

export interface Meter {
  used: number;
  limit: number;
  /** Whole percent, at most 100. */
  percent: number;
  level: Level;
}

const meter = (used: number, limit: number | null | undefined): Meter | null =>
  limit ? { used, limit, percent: percentOf(used, limit), level: levelOf(used, limit) } : null;

/** The quota and daily-cap meters of a usage report (null for a limit the account does not have). */
export function meters(u: MyUsage): { quota: Meter | null; daily: Meter | null; maxFile: number | null } {
  return {
    quota: meter(u.stored_bytes, u.limits?.quota_bytes),
    daily: meter(u.daily?.used_bytes ?? u.uploads.bytes_24h, u.limits?.daily_upload_bytes),
    maxFile: u.limits?.max_file_bytes ?? null,
  };
}

/** The warning for a meter at 80 % and 95 %, or null below that. `what` is "storage" or "daily upload limit". */
export function warningFor(m: Meter | null, what: "storage" | "daily upload limit"): string | null {
  if (!m) return null;
  switch (m.level) {
    case "ok":
      return null;
    case "warn":
      return `You have used ${m.percent}% of your ${what}.`;
    case "critical":
      return what === "storage"
        ? `Your storage is almost full: ${m.percent}% used. Delete photos and empty the trash, or ask the administrator for more.`
        : `You are almost at your daily upload limit: ${m.percent}% used.`;
    case "full":
      return what === "storage"
        ? "Your storage is full. New uploads are refused until you free up space (the trash counts until it is emptied)."
        : "You have reached your daily upload limit. Uploads resume as earlier ones leave the 24 hour window.";
  }
}

/** "3:04 PM today", "3:04 PM tomorrow" or "Jun 3, 3:04 PM": when uploads can resume. */
export function resumeText(iso: string, now: Date = new Date()): string {
  const at = new Date(iso);
  const time = at.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((day(at) - day(now)) / 86_400_000);
  if (diff === 0) return `${time} today`;
  if (diff === 1) return `${time} tomorrow`;
  return `${at.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
}

export type UploadBlock = { code: "quota_exceeded" | "daily_cap_exceeded"; message: string; retryAt: string | null };

/** What the web uploader tells the user after a quota or daily cap refusal. */
export function blockText(b: UploadBlock, now: Date = new Date()): string {
  if (b.code === "daily_cap_exceeded") {
    return b.retryAt
      ? `Daily upload limit reached. Uploads can resume at ${resumeText(b.retryAt, now)}.`
      : "Daily upload limit reached. Try again later.";
  }
  return "Not enough storage for these files. Delete photos and empty the trash, or ask the administrator for more space.";
}

/** One short line of the limits for the upload tray: "Storage 4.0 GB of 5.0 GB · today 600 MB of 1.0 GB". */
export function limitsLine(u: MyUsage): string | null {
  const m = meters(u);
  const parts: string[] = [];
  if (m.quota) parts.push(`Storage ${bytesLabel(m.quota.used)} of ${bytesLabel(m.quota.limit)}`);
  if (m.daily) parts.push(`last 24 h ${bytesLabel(m.daily.used)} of ${bytesLabel(m.daily.limit)}`);
  if (m.maxFile) parts.push(`files up to ${bytesLabel(m.maxFile)}`);
  return parts.length ? parts.join(" · ") : null;
}

/** A limit as the admin form shows it: gigabytes without trailing zeros, "" for unlimited. */
export function gigabytesField(bytes: number | null | undefined): string {
  if (!bytes) return "";
  const gb = bytes / 1024 ** 3;
  return String(Math.round(gb * 1000) / 1000);
}

/** One account's limits in words for the admin list ("5.0 GB storage · 1.0 GB a day · files up to 200 MB"), or "No limits". */
export function limitsWords(l: Partial<Limits>): string {
  const parts: string[] = [];
  if (l.quota_bytes) parts.push(`${bytesLabel(l.quota_bytes)} storage`);
  if (l.daily_upload_bytes) parts.push(`${bytesLabel(l.daily_upload_bytes)} a day`);
  if (l.max_file_bytes) parts.push(`files up to ${bytesLabel(l.max_file_bytes)}`);
  return parts.length ? parts.join(" · ") : "No limits";
}
