// Google Takeout in the browser: send the *names* of an unzipped export to the server (which pairs each photo with its JSON
// sidecar with the same code the command line uses), show what would happen, then upload with each photo's metadata.
// Framework-free so it can be tested without React.
import type { CheckRequest, TakeoutPlan, TakeoutPlanItem } from "./api";
import { isUploadable } from "./upload/files";

/** The relative path of a file chosen with a folder picker or dropped (the picker's, else its plain name). */
export const relPath = (f: File): string => (f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name;

/** Names per request. The server takes 100,000; leave room. */
export const BATCH_PATHS = 50_000;

/** Group file names by folder, then into requests that never split a folder (a sidecar must travel with its photo). */
export function batches(paths: string[], limit = BATCH_PATHS): string[][] {
  const byDir = new Map<string, string[]>();
  for (const p of paths) {
    const i = p.lastIndexOf("/");
    const dir = i < 0 ? "" : p.slice(0, i);
    const list = byDir.get(dir);
    if (list) list.push(p);
    else byDir.set(dir, [p]);
  }
  const out: string[][] = [];
  let cur: string[] = [];
  for (const files of byDir.values()) {
    // A single folder over the limit (a library of a hundred thousand photos in one year) is cut, in name order so that a
    // photo and its sidecar, which sort together, rarely land in different requests.
    const sorted = files.length > limit ? [...files].sort() : files;
    for (let i = 0; i < sorted.length; i += limit) {
      const chunk = sorted.slice(i, i + limit);
      if (cur.length + chunk.length > limit && cur.length > 0) {
        out.push(cur);
        cur = [];
      }
      cur = cur.concat(chunk);
    }
  }
  if (cur.length > 0) out.push(cur);
  return out;
}

const empty = (): TakeoutPlan => ({ photos: [], videos: [], other: [], trashed: [], albums: {}, unused_sidecars: [] });

/** Merge the answers of several requests. */
export function merge(plans: TakeoutPlan[]): TakeoutPlan {
  const out = empty();
  for (const p of plans) {
    out.photos.push(...p.photos);
    out.videos.push(...p.videos);
    out.other.push(...p.other);
    out.trashed.push(...p.trashed);
    out.unused_sidecars.push(...p.unused_sidecars);
    for (const [k, n] of Object.entries(p.albums)) out.albums[k] = (out.albums[k] ?? 0) + n;
  }
  return out;
}

/** Ask the server to pair the names of `files`. Hidden and system files never leave the browser. */
export async function planFiles(
  files: File[],
  ask: (paths: string[]) => Promise<TakeoutPlan>,
  onProgress?: (done: number, total: number) => void,
): Promise<TakeoutPlan> {
  const paths = files.map(relPath).filter((p) => !p.split("/").some((s) => s.startsWith(".") || s === "__MACOSX"));
  const groups = batches(paths);
  const plans: TakeoutPlan[] = [];
  for (const g of groups) {
    plans.push(await ask(g));
    onProgress?.(plans.length, groups.length);
  }
  return merge(plans);
}

export interface Preview {
  photos: number;
  videos: number;
  other: number;
  trashed: number;
  albums: { name: string; photos: number }[];
  withoutSidecar: number;
  edited: number;
  /** Photos with a sidecar that says anything to apply (always the ones with a sidecar; the server decides what applies). */
  withSidecar: number;
}

export function previewOf(p: TakeoutPlan): Preview {
  const withoutSidecar = p.photos.filter((i) => i.sidecar === null).length;
  return {
    photos: p.photos.length,
    videos: p.videos.length,
    other: p.other.length,
    trashed: p.trashed.length,
    albums: Object.entries(p.albums)
      .map(([name, photos]) => ({ name, photos }))
      .sort((a, b) => a.name.localeCompare(b.name)),
    withoutSidecar,
    edited: p.photos.filter((i) => i.edited).length,
    withSidecar: p.photos.length - withoutSidecar,
  };
}

export interface Options {
  favorites: boolean;
  albums: boolean;
}

/** The files to upload and, for each, how to build its `takeout` field (read lazily: the sidecars are small, but there may be 50,000). */
export function uploadSet(
  plan: TakeoutPlan,
  files: File[],
  opts: Options,
): { files: File[]; takeoutOf: (f: File) => Promise<CheckRequest["takeout"]> } {
  const byPath = new Map(files.map((f) => [relPath(f), f]));
  const items = new Map<File, TakeoutPlanItem>();
  const photos: File[] = [];
  for (const it of plan.photos) {
    const f = byPath.get(it.path);
    // A photo the browser would not upload anyway (an extension it does not know) is not sent.
    if (f && isUploadable(f.name, it.path)) {
      photos.push(f);
      items.set(f, it);
    }
  }
  return {
    files: photos,
    takeoutOf: async (f) => {
      const it = items.get(f);
      if (!it) return undefined;
      let sidecar: string | undefined;
      if (it.sidecar) {
        const s = byPath.get(it.sidecar);
        // Real sidecars are 1-3 KB; the server ignores anything over 64 KB, so do not send it.
        if (s && s.size <= 64 * 1024) sidecar = await s.text();
      }
      const album = opts.albums && it.album ? it.album : undefined;
      if (sidecar === undefined && album === undefined) return undefined;
      return { sidecar, album, favorites: opts.favorites };
    },
  };
}
