// Browser upload queue: hash -> check (proxy transport) -> PUT missing parts -> complete.
// Framework-free so it can be tested without React; UploadProvider adapts it to the UI.
import { ApiError, type Api, type CheckRequest, type DonePart } from "../api";
import { contentTypeOf } from "./files";
import type { UploadBlock } from "../limitsText";

/** `stopped`: not started because the account's quota or daily cap refused an earlier file (see `UploadItem.block`). */
export type UploadStatus = "queued" | "hashing" | "checking" | "uploading" | "done" | "duplicate" | "failed" | "stopped";

export interface UploadItem {
  id: number;
  file: File;
  status: UploadStatus;
  /** 0..1 within the current phase (hashing or uploading). */
  progress: number;
  error?: string;
  assetId?: string;
  /** Set on the file a quota or daily cap refused and on every file stopped behind it. */
  block?: UploadBlock;
  /** Google Takeout: builds the metadata sent with this file's check call (read when the file is reached, not up front). */
  takeout?: () => Promise<CheckRequest["takeout"]>;
}

export interface QueueDeps {
  api: Pick<Api, "checkUpload" | "putPart" | "completeUpload">;
  hash: (file: File, onProgress: (done: number) => void) => Promise<string>;
  /** Backoff before retry `attempt` (1-based); tests pass 0. */
  delay?: (attempt: number) => Promise<void>;
  fileConcurrency?: number;
  partConcurrency?: number;
  maxAttempts?: number;
}

const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const defaultDelay = (attempt: number) => wait(500 * 2 ** (attempt - 1));

/** Server-side status codes that will not get better by retrying the same bytes. */
const retryable = (e: unknown) => !(e instanceof ApiError) || e.status === 0 || e.status === 408 || e.status === 429 || e.status >= 500;

class Semaphore {
  private waiters: (() => void)[] = [];
  private free: number;
  constructor(n: number) {
    this.free = n;
  }
  async run<T>(fn: () => Promise<T>): Promise<T> {
    if (this.free > 0) this.free--;
    else await new Promise<void>((r) => this.waiters.push(r));
    try {
      return await fn();
    } finally {
      const next = this.waiters.shift();
      if (next) next();
      else this.free++;
    }
  }
}

export interface Summary {
  total: number;
  finished: number; // done + duplicate + failed
  done: number;
  duplicate: number;
  failed: number;
  /** Files not started because a quota or daily cap refused an earlier one. */
  stopped: number;
  /** The quota or daily cap refusal that stopped the batch, if any. */
  blocked: UploadBlock | null;
  active: boolean;
  /** 0..1 over all files, by bytes. */
  progress: number;
}

export class UploadQueue {
  private items: UploadItem[] = [];
  private nextId = 1;
  private running = 0;
  private listeners = new Set<() => void>();
  private doneListeners = new Set<(item: UploadItem) => void>();
  private snapshot: readonly UploadItem[] = [];
  private readonly parts: Semaphore;
  private readonly fileLimit: number;
  private readonly maxAttempts: number;
  private readonly delay: (attempt: number) => Promise<void>;

  constructor(private deps: QueueDeps) {
    this.fileLimit = deps.fileConcurrency ?? 2;
    this.parts = new Semaphore(deps.partConcurrency ?? 3);
    this.maxAttempts = deps.maxAttempts ?? 4;
    this.delay = deps.delay ?? defaultDelay;
  }

  // useSyncExternalStore contract: subscribe + a stable snapshot between changes.
  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => void this.listeners.delete(l);
  };
  getSnapshot = () => this.snapshot;

  /** Called once for every file that ends up in the library (new upload; not for duplicates). */
  onAssetAdded(l: (item: UploadItem) => void) {
    this.doneListeners.add(l);
    return () => void this.doneListeners.delete(l);
  }

  /** `takeoutOf`: for a Google Takeout import, how to build the metadata that travels with each file (see `takeout.ts`). */
  add(files: File[], takeoutOf?: (f: File) => Promise<CheckRequest["takeout"]>) {
    for (const file of files) {
      this.items.push({
        id: this.nextId++,
        file,
        status: "queued",
        progress: 0,
        ...(takeoutOf ? { takeout: () => takeoutOf(file) } : {}),
      });
    }
    this.emit();
    this.pump();
  }

  retry(id: number) {
    const it = this.items.find((i) => i.id === id);
    if (!it || (it.status !== "failed" && it.status !== "stopped")) return;
    this.patch(it, { status: "queued", progress: 0, error: undefined, block: undefined });
    this.pump();
  }

  /** Queue every file a quota or daily cap stopped again (after space was freed or the day rolled over). */
  retryBlocked() {
    this.items = this.items.map((i) =>
      i.block && (i.status === "failed" || i.status === "stopped")
        ? { ...i, status: "queued", progress: 0, error: undefined, block: undefined }
        : i,
    );
    this.emit();
    this.pump();
  }

  /** Drop finished entries (done, duplicate, failed, stopped) from the list. */
  clearFinished() {
    this.items = this.items.filter((i) => !FINISHED.includes(i.status));
    this.emit();
  }

  get active() {
    return this.items.some((i) => ["queued", "hashing", "checking", "uploading"].includes(i.status));
  }

  summary(): Summary {
    return summarize(this.items);
  }

  private emit() {
    // Items are mutated copy-on-write in patch(), so a shallow copy is a valid new snapshot.
    this.snapshot = [...this.items];
    this.listeners.forEach((l) => l());
  }

  private patch(it: UploadItem, p: Partial<UploadItem>) {
    const next = { ...it, ...p };
    this.items = this.items.map((i) => (i.id === it.id ? next : i));
    this.emit();
    return next;
  }

  private current(id: number) {
    return this.items.find((i) => i.id === id);
  }

  private pump() {
    while (this.running < this.fileLimit) {
      const next = this.items.find((i) => i.status === "queued");
      if (!next) return;
      this.running++;
      this.patch(next, { status: "hashing", progress: 0 });
      void this.process(next.id).finally(() => {
        this.running--;
        this.pump();
      });
    }
  }

  private async process(id: number) {
    const set = (p: Partial<UploadItem>) => {
      const cur = this.current(id);
      if (cur) this.patch(cur, p);
    };
    const file = this.current(id)!.file;
    const takeoutOf = this.current(id)!.takeout;
    try {
      if (file.size === 0) throw new Error("The file is empty.");
      const sha256 = await this.deps.hash(file, (d) => set({ progress: d / file.size }));
      set({ status: "checking", progress: 1 });
      const takeout = await takeoutOf?.();
      const check = await this.deps.api.checkUpload({
        sha256,
        size_bytes: file.size,
        filename: file.name,
        content_type: contentTypeOf(file),
        file_modified_at: new Date(file.lastModified).toISOString(),
        transport: "proxy",
        ...(takeout ? { takeout } : {}),
      });
      if (check.status === "exists") {
        set({ status: "duplicate", progress: 1, assetId: check.asset_id });
        return;
      }
      const have = new Map<number, number>(); // bytes sent per part, for the progress bar
      const partLen = (n: number) => Math.min(check.part_size, file.size - (n - 1) * check.part_size);
      for (const d of check.uploaded) have.set(d.number, partLen(d.number));
      const report = () => {
        let sent = 0;
        have.forEach((v) => (sent += v));
        set({ status: "uploading", progress: Math.min(1, sent / file.size) });
      };
      report();
      const etags: DonePart[] = [...check.uploaded];
      await Promise.all(
        check.parts.map((p) =>
          this.parts.run(async () => {
            const start = (p.number - 1) * check.part_size;
            const blob = file.slice(start, start + partLen(p.number));
            for (let attempt = 1; ; attempt++) {
              try {
                const { etag } = await this.deps.api.putPart(p.url, blob, {
                  onProgress: (loaded) => {
                    have.set(p.number, Math.min(loaded, blob.size));
                    report();
                  },
                });
                have.set(p.number, blob.size);
                report();
                etags.push({ number: p.number, etag });
                return;
              } catch (e) {
                have.set(p.number, 0);
                if (attempt >= this.maxAttempts || !retryable(e)) throw e;
                await this.delay(attempt);
              }
            }
          }),
        ),
      );
      const { asset_id } = await this.deps.api.completeUpload(
        check.upload_id,
        etags.sort((a, b) => a.number - b.number),
      );
      const done = this.current(id);
      if (done) {
        const item = this.patch(done, { status: "done", progress: 1, assetId: asset_id });
        this.doneListeners.forEach((l) => l(item));
      }
    } catch (e) {
      const block = blockOf(e);
      if (block) {
        // Every further file would be refused too: stop the ones not started and say why, once.
        this.items = this.items.map((i) =>
          i.status === "queued" ? { ...i, status: "stopped", error: block.message, block } : i,
        );
        set({ status: "failed", error: block.message, block });
        return;
      }
      set({ status: "failed", error: e instanceof Error ? e.message : "Upload failed." });
    }
  }
}

const FINISHED: UploadStatus[] = ["done", "duplicate", "failed", "stopped"];

/** A refusal by the account's storage quota or daily cap (not a single file that is too big: that fails alone). */
function blockOf(e: unknown): UploadBlock | null {
  if (!(e instanceof ApiError) || (e.code !== "quota_exceeded" && e.code !== "daily_cap_exceeded")) return null;
  const at = e.details?.retry_at;
  return { code: e.code, message: e.message, retryAt: typeof at === "string" ? at : null };
}

export function summarize(items: readonly UploadItem[]): Summary {
  let done = 0, duplicate = 0, failed = 0, stopped = 0, bytes = 0, got = 0;
  let blocked: UploadBlock | null = null;
  for (const i of items) {
    if (i.status === "done") done++;
    else if (i.status === "duplicate") duplicate++;
    else if (i.status === "failed") failed++;
    else if (i.status === "stopped") stopped++;
    if (i.block && (i.status === "failed" || i.status === "stopped")) blocked ??= i.block;
    bytes += i.file.size;
    got += i.file.size * (FINISHED.includes(i.status) ? 1 : i.status === "uploading" ? i.progress : 0);
  }
  const finished = done + duplicate + failed + stopped;
  return {
    total: items.length,
    finished,
    done,
    duplicate,
    failed,
    stopped,
    blocked,
    active: finished < items.length,
    progress: bytes ? got / bytes : 1,
  };
}
