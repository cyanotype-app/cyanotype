import type { IntegrityAction, IntegrityFinding, IntegrityKind, IntegrityRun, IntegritySummary } from "./api";
import { bytesLabel } from "./adminText";
import { ago, countLabel } from "./storageText";

export const KIND_LABEL: Record<IntegrityKind, string> = {
  missing_original: "Missing originals",
  untracked_object: "Untracked objects",
  checksum_mismatch: "Checksum mismatches",
};

export const KINDS: IntegrityKind[] = ["missing_original", "untracked_object", "checksum_mismatch"];

/** A short hash for display: the first 12 characters. */
export const shortHash = (h: string | undefined): string => (h ? h.slice(0, 12) : "unknown");

/** One line saying what is wrong with a finding. */
export function findingTitle(f: IntegrityFinding): string {
  const d = f.detail;
  if (f.kind === "missing_original") return "The original is gone from the bucket.";
  if (f.kind === "untracked_object") {
    if (d.reason === "duplicate_content") return "An extra copy of a photo that is indexed under another key.";
    if (d.reason === "unrecognised_key") return "An object whose key is not an original's.";
    return d.owner_known === false ? "An object of an account that does not exist here." : "An object no photo points at.";
  }
  if (d.reason === "size") return `The file is ${bytesLabel(d.actual_bytes ?? 0)}, the library says ${bytesLabel(d.size_bytes ?? 0)}.`;
  return "The file's bytes no longer match the stored checksum.";
}

export interface FindingAction {
  action: IntegrityAction;
  label: string;
  danger?: boolean;
}

/** The actions offered for a finding, in order. */
export function actionsFor(f: IntegrityFinding): FindingAction[] {
  const d = f.detail;
  const out: FindingAction[] = [];
  if (f.kind === "missing_original" || f.kind === "checksum_mismatch") {
    // Offered unless the bucket is known to hold no earlier version.
    if (d.previous_versions !== 0) out.push({ action: "restore_version", label: "Restore from a previous version" });
    if (f.kind === "missing_original") out.push({ action: "trash", label: "Move to trash" });
    out.push({ action: "acknowledge", label: f.kind === "missing_original" ? "Mark lost" : "Leave it" });
  } else {
    if (d.reason === "no_row" && d.owner_known !== false) out.push({ action: "restore_photo", label: "Restore as photo" });
    out.push({ action: "delete_object", label: "Delete object", danger: true });
    out.push({ action: "acknowledge", label: "Leave it" });
  }
  return out;
}

/** What an action did, for the note under the list. */
export function actedLine(action: IntegrityAction, changed: boolean): string {
  if (!changed) return "Already done.";
  switch (action) {
    case "restore_version":
      return "Restored from a previous version.";
    case "trash":
      return "Moved to the trash.";
    case "acknowledge":
      return "Left as it is. It is no longer counted.";
    case "restore_photo":
      return "Made a photo of it. Thumbnails will follow.";
    case "delete_object":
      return "Object deleted.";
  }
}

/** The last run as a sentence. */
export function runLine(r: IntegrityRun | null, now: number = Date.now()): string {
  if (!r) return "The check has not run yet. It runs nightly, or press Run check now.";
  if (r.state === "running") return `Running since ${ago(r.started_at, now)}.`;
  if (r.state === "failed") return `The last check stopped ${ago(r.finished_at ?? r.started_at, now)}: ${r.error ?? "no reason recorded"}`;
  const requeued = r.derived_requeued ? ` ${countLabel(r.derived_requeued, "thumbnail")} queued to be made again.` : "";
  return `Last check ${ago(r.finished_at ?? r.started_at, now)}: ${countLabel(r.objects_listed, "object")} listed, ${countLabel(r.assets_checked, "photo")} compared, ${bytesLabel(r.bytes_hashed)} re-read.${requeued}`;
}

/** How much of the library the rolling checksum sample has read at least once. */
export function coverageLine(c: IntegritySummary["coverage"], now: number = Date.now()): string {
  if (c.assets === 0) return "No photos yet.";
  const pct = Math.floor((c.verified / c.assets) * 100);
  const old = c.oldest_verified_at ? ` The oldest was read ${ago(c.oldest_verified_at, now)}.` : "";
  return `Checksums read for ${c.verified.toLocaleString("en-US")} of ${c.assets.toLocaleString("en-US")} photos (${pct}%).${old}`;
}
