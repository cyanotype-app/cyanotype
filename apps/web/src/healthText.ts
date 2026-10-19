import type { Health } from "./api";
import { bytesLabel } from "./adminText";
import { ago } from "./storageText";

/** "45 s", "3 min", "2 h 5 min", "3 d". */
export function duration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  return `${Math.floor(h / 24)} d`;
}

export type Level = "ok" | "warn" | "bad" | "none";

/** A dump older than this is a warning: the backup runs nightly, so a day and a half means a night was missed. */
const BACKUP_STALE_HOURS = 36;

/** The backup card's one line and how worried to look. */
export function backupStatus(b: Health["backup"], now: number = Date.now()): { level: Level; text: string } {
  if (b.error) return { level: "bad", text: b.error };
  if (!b.last_at) return { level: "bad", text: `No backup found under ${b.prefix}/.` };
  const age = (now - new Date(b.last_at).getTime()) / 3_600_000;
  const size = b.last_size_bytes != null ? `, ${bytesLabel(b.last_size_bytes)}` : "";
  const text = `Last backup ${ago(b.last_at, now)}${size}. ${b.count ?? 0} kept.`;
  return { level: age > BACKUP_STALE_HOURS ? "warn" : "ok", text };
}

/** A worker's memory as one line: resident size and how much of the decode budget is in use. */
export function memoryLine(w: Health["workers"][number]): string {
  const parts: string[] = [];
  if (w.rss_bytes != null) parts.push(`${bytesLabel(w.rss_bytes)} resident`);
  if (w.budget_total_bytes) parts.push(`decode budget ${bytesLabel(w.budget_used_bytes ?? 0)} of ${bytesLabel(w.budget_total_bytes)}`);
  return parts.length ? parts.join(" · ") : "no numbers yet";
}

/** Waiting, running and failed jobs over every kind. */
export function queueTotals(q: Health["queue"]): { queued: number; running: number; failed: number } {
  return q.reduce(
    (t, k) => ({ queued: t.queued + k.queued, running: t.running + k.running, failed: t.failed + k.failed }),
    { queued: 0, running: 0, failed: 0 },
  );
}

/** The kinds that have failed jobs, with their counts. */
export const failedKinds = (q: Health["queue"]): { kind: string; failed: number }[] =>
  q.filter((k) => k.failed > 0).map((k) => ({ kind: k.kind, failed: k.failed }));
