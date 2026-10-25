// Preview addresses are presigned and short-lived, so a slideshow asks for them just before a photo is due, a few at a time,
// instead of trusting the ones that came with a list page minutes (or, for a show of hours, long) ago.
import { api, type PreviewInfo } from "../api";

/** Younger than this, an address is used again; the server's own life is longer (minutes), this leaves a wide margin. */
const MAX_AGE_MS = 3 * 60 * 1000;
/** Most addresses kept (a slideshow needs the next few and the last one). */
const MAX_ENTRIES = 64;
/** Most photos one request names (the server's limit). */
const BATCH = 24;

export class PreviewCache {
  private entries = new Map<string, { info: PreviewInfo | null; at: number }>();
  private inflight = new Map<string, Promise<void>>();

  constructor(
    private fetchPreviews: (ids: readonly string[]) => Promise<{ items: PreviewInfo[] }> = api.previews,
    private now: () => number = Date.now,
  ) {}

  private fresh(id: string): boolean {
    const e = this.entries.get(id);
    return !!e && this.now() - e.at < MAX_AGE_MS;
  }

  /**
   * The preview of `id`, or null when the photo has none to show (gone, hidden, not rendered). `ahead` are the photos due
   * after it: any that are missing are fetched in the same request.
   */
  async get(id: string, ahead: readonly string[] = []): Promise<PreviewInfo | null> {
    const waiting = this.inflight.get(id);
    if (waiting) await waiting;
    if (!this.fresh(id)) {
      const want = [id, ...ahead.filter((a) => a !== id && !this.fresh(a) && !this.inflight.has(a))].slice(0, BATCH);
      const run = this.fetch(want);
      for (const w of want) this.inflight.set(w, run);
      try {
        await run;
      } finally {
        for (const w of want) this.inflight.delete(w);
      }
    }
    return this.entries.get(id)?.info ?? null;
  }

  private async fetch(ids: string[]): Promise<void> {
    const res = await this.fetchPreviews(ids);
    const at = this.now();
    const got = new Map(res.items.map((i) => [i.id, i]));
    for (const id of ids) {
      this.entries.delete(id);
      this.entries.set(id, { info: got.get(id) ?? null, at });
    }
    while (this.entries.size > MAX_ENTRIES) this.entries.delete(this.entries.keys().next().value as string);
  }

  /** Forget an address (the picture it named could not be loaded: it may have expired). */
  forget(id: string) {
    this.entries.delete(id);
  }

  clear() {
    this.entries.clear();
    this.inflight.clear();
  }
}
