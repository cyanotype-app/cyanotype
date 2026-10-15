import type { AssetSummary, PairedFile } from "./api";

/** "3.4 MB" style size for the pair info. */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 10 ? Math.round(v) : v.toFixed(1)} ${units[u]}`;
}

/** What the visible file of a pair is called: JPEG, HEIC or AVIF (by content type, then extension). */
function primaryFormat(primary: Pick<AssetSummary, "content_type" | "original_name">): string {
  const probe = `${primary.content_type} ${primary.original_name.split(".").pop() ?? ""}`.toLowerCase();
  if (/heic|heif|hif/.test(probe)) return "HEIC";
  if (/avif/.test(probe)) return "AVIF";
  return "JPEG";
}

/** "RAW + JPEG" for a stacked photo. */
export const pairLabel = (primary: Pick<AssetSummary, "content_type" | "original_name">, paired: readonly PairedFile[]) =>
  paired.length > 0 ? `RAW + ${primaryFormat(primary)}` : "";
