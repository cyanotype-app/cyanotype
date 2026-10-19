import type { AdminTask, BucketReport } from "./api";
import { bytesLabel } from "./adminText";

/** "1 photo", "2 photos". */
export const countLabel = (n: number, one: string, many = `${one}s`): string => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

/** The two parts of what an account stores, as shares of the bar (they sum to 100, or both are 0 when nothing is stored). */
export function barShares(originals: number, trash: number): { photos: number; trash: number } {
  const total = originals + trash;
  if (total <= 0) return { photos: 0, trash: 0 };
  let photos = Math.round((originals / total) * 100);
  // A non-empty part keeps at least a sliver so it stays visible.
  if (trash > 0 && photos === 100) photos = 99;
  if (originals > 0 && photos === 0) photos = 1;
  return { photos, trash: 100 - photos };
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "just now", "5 minutes ago", "3 hours ago", "2 days ago", or the date for older; "never" without a time. */
export function ago(iso: string | null, now: number = Date.now()): string {
  if (!iso) return "never";
  const ms = now - new Date(iso).getTime();
  if (ms < MINUTE) return "just now";
  const plural = (n: number, u: string) => `${n} ${u}${n === 1 ? "" : "s"} ago`;
  if (ms < HOUR) return plural(Math.floor(ms / MINUTE), "minute");
  if (ms < DAY) return plural(Math.floor(ms / HOUR), "hour");
  if (ms < 30 * DAY) return plural(Math.floor(ms / DAY), "day");
  return new Date(iso).toLocaleDateString();
}

/** What to say about the derived (thumbnail and preview) size: exact when every file is measured, else a lower bound. */
export function derivedLabel(bytes: number, complete: boolean): string {
  return complete ? bytesLabel(bytes) : `at least ${bytesLabel(bytes)}`;
}

/** One line for the nightly check of the counters. */
export function reconcileLine(r: { at: string; users_checked: number; drifted_users: number } | null, now: number = Date.now()): string {
  if (!r) return "The nightly check of these numbers has not run yet.";
  const when = ago(r.at, now);
  return r.drifted_users === 0
    ? `Numbers checked against the files ${when}: all matched.`
    : `Numbers checked against the files ${when}: ${countLabel(r.drifted_users, "account")} had drifted and ${r.drifted_users === 1 ? "was" : "were"} corrected.`;
}

/** The state of a bucket check task as a sentence. */
export function bucketTaskLine(t: AdminTask<BucketReport> | null, now: number = Date.now()): string {
  if (!t) return "Not run yet.";
  switch (t.state) {
    case "queued":
      return "Waiting for the worker to pick it up…";
    case "running":
      return "Listing the bucket…";
    case "failed":
      return `Failed: ${t.error ?? "unknown error"}`;
    case "done":
      return `Finished ${ago(t.finished_at, now)}.`;
  }
}

/** Accounts of a report whose bucket originals differ from the database, as "alice@x: bucket 1.0 GB, database 1.1 GB". */
export function mismatchLine(m: BucketReport["mismatched"][number]): string {
  return `${m.email}: bucket ${bytesLabel(m.bucket_originals_bytes)}, database ${bytesLabel(m.db_originals_bytes)}`;
}
