// Words of the near-duplicate review: resolution, metadata, the scan's state and the result of keeping one. Pure; unit tested.
import type { DuplicateMember, DuplicateScan } from "./api";
import { fileSize } from "./pairText";

/** "4000 × 3000 · 12 MP", or "Unknown size". */
export function resolutionLine(w: number | null, h: number | null): string {
  if (!w || !h) return "Unknown size";
  const mp = (w * h) / 1e6;
  return `${w} × ${h} · ${mp >= 10 ? Math.round(mp) : mp.toFixed(1)} MP`;
}

/** What the file carries besides pixels: "34 EXIF fields, location, caption, 2 tags" or "No metadata". */
export function metadataLine(m: Pick<DuplicateMember, "exif_fields" | "has_location" | "has_caption" | "tag_count">): string {
  const parts: string[] = [];
  if (m.exif_fields > 0) parts.push(`${m.exif_fields} EXIF field${m.exif_fields === 1 ? "" : "s"}`);
  if (m.has_location) parts.push("location");
  if (m.has_caption) parts.push("caption");
  if (m.tag_count > 0) parts.push(`${m.tag_count} tag${m.tag_count === 1 ? "" : "s"}`);
  return parts.length ? parts.join(", ") : "No metadata";
}

/** The size of a file and how it compares with the largest of the group: "5.0 MB" / "90 KB (2 % of the largest)". */
export function sizeLine(m: Pick<DuplicateMember, "size_bytes">, all: readonly Pick<DuplicateMember, "size_bytes">[]): string {
  const largest = Math.max(...all.map((x) => x.size_bytes));
  if (m.size_bytes >= largest || largest === 0) return fileSize(m.size_bytes);
  return `${fileSize(m.size_bytes)} (${Math.max(1, Math.round((m.size_bytes * 100) / largest))} % of the largest)`;
}

/** The state of the library check, in a line. */
export function scanLine(s: DuplicateScan | undefined, now: Date = new Date()): string {
  if (!s || s.state === "never") return "The library has not been checked for duplicates yet.";
  if (s.state === "running") return "Checking your library for duplicates…";
  if (s.state === "wanted") return s.manual ? "Waiting for the check to start…" : "A check will run soon, after your latest uploads settle.";
  if (s.error) return `The last check failed: ${s.error}`;
  const found = s.groups === 0 ? "no duplicates found" : `${s.groups} ${s.groups === 1 ? "group" : "groups"} found`;
  const when = s.finished_at ? ago(new Date(s.finished_at), now) : "";
  return `Last checked ${when}${s.photos !== null && s.photos !== undefined ? ` (${s.photos.toLocaleString("en-US")} photos)` : ""}: ${found}.`;
}

function ago(t: Date, now: Date): string {
  const s = Math.max(0, Math.round((now.getTime() - t.getTime()) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86_400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86_400)} d ago`;
}

/** "Keep this one and trash 2 others". */
export const keepLabel = (others: number) => `Keep selected, trash ${others} ${others === 1 ? "other" : "others"}`;

/** The toast after keeping one: what moved to the trash and what came across. */
export function keptMessage(r: { trashed: readonly string[]; merged: Record<string, unknown> }): string {
  const n = r.trashed.length;
  const m = r.merged;
  const carried: string[] = [];
  if (typeof m.albums === "number" && m.albums > 0) carried.push(`${m.albums} album${m.albums === 1 ? "" : "s"}`);
  if (typeof m.tags === "number" && m.tags > 0) carried.push(`${m.tags} tag${m.tags === 1 ? "" : "s"}`);
  if (m.favorite) carried.push("the favorite");
  if (typeof m.rating === "number") carried.push(`${m.rating} stars`);
  if (m.caption) carried.push("captions");
  if (m.location) carried.push("the location");
  if (m.date) carried.push("the date");
  const moved = `${n} ${n === 1 ? "photo" : "photos"} moved to the trash`;
  return carried.length ? `${moved}. Merged into the one you kept: ${carried.join(", ")}.` : `${moved}.`;
}
