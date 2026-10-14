// Which files the browser uploader accepts. MIME types are useless for this (browsers report "" for RAW), so
// it goes by extension; hidden and OS bookkeeping files are skipped.

export const IMAGE_EXTENSIONS: ReadonlySet<string> = new Set([
  "jpg", "jpeg", "png", "webp", "heic", "heif", "avif",
  "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "rwl", "pef", "srw",
  "3fr", "iiq", "erf", "kdc", "mrw", "x3f", "crw",
]);

/** For the file input's `accept` attribute (extensions work where MIME types would not). */
export const ACCEPT = Array.from(IMAGE_EXTENSIONS, (e) => `.${e}`).join(",");

const SYSTEM_FILES = new Set(["thumbs.db", "desktop.ini", "ehthumbs.db"]);

export function extensionOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i <= 0 ? "" : name.slice(i + 1).toLowerCase();
}

/** `path` may be a relative path (`webkitRelativePath`): any hidden or `__MACOSX` segment excludes the file. */
export function isUploadable(name: string, path: string = name): boolean {
  const segments = path.split("/").filter(Boolean);
  if (segments.some((s) => s.startsWith(".") || s === "__MACOSX")) return false;
  const base = name.toLowerCase();
  if (base.startsWith("._") || SYSTEM_FILES.has(base)) return false;
  return IMAGE_EXTENSIONS.has(extensionOf(name));
}

export interface Selection {
  accepted: File[];
  /** Number of files left out (not an image extension, or hidden/system). */
  skipped: number;
}

export function filterFiles(files: Iterable<File>): Selection {
  const accepted: File[] = [];
  let skipped = 0;
  for (const f of files) {
    const rel = (f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name;
    if (isUploadable(f.name, rel)) accepted.push(f);
    else skipped++;
  }
  return { accepted, skipped };
}

/** Browsers report "" for types they do not know (RAW); the server wants some content type. */
export function contentTypeOf(f: File): string {
  return f.type || "application/octet-stream";
}

interface FsEntry {
  isFile: boolean;
  isDirectory: boolean;
  name: string;
  fullPath: string;
  file?: (ok: (f: File) => void, err: (e: unknown) => void) => void;
  createReader?: () => { readEntries: (ok: (e: FsEntry[]) => void, err: (e: unknown) => void) => void };
}

async function walk(entry: FsEntry, out: File[]): Promise<void> {
  if (entry.isFile && entry.file) {
    const f = await new Promise<File>((ok, err) => entry.file!(ok, err));
    // Dropped files have no webkitRelativePath; remember the path so hidden folders are still skipped.
    Object.defineProperty(f, "webkitRelativePath", { value: entry.fullPath.replace(/^\//, "") });
    out.push(f);
  } else if (entry.isDirectory && entry.createReader) {
    const reader = entry.createReader();
    for (;;) {
      // readEntries returns batches (about 100); an empty batch means done.
      const batch = await new Promise<FsEntry[]>((ok, err) => reader.readEntries(ok, err));
      if (batch.length === 0) break;
      for (const e of batch) await walk(e, out);
    }
  }
}

/** Files from a drop, descending into dropped folders where the browser exposes them. */
export async function filesFromDrop(dt: DataTransfer): Promise<File[]> {
  const entries: FsEntry[] = [];
  for (const item of Array.from(dt.items ?? [])) {
    if (item.kind !== "file") continue;
    const e = (item as DataTransferItem & { webkitGetAsEntry?: () => FsEntry | null }).webkitGetAsEntry?.();
    if (e) entries.push(e);
  }
  if (entries.length === 0) return Array.from(dt.files);
  const out: File[] = [];
  for (const e of entries) await walk(e, out);
  return out;
}
