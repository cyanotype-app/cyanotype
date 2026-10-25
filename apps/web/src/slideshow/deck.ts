// The order a slideshow runs through a set of photos, and the stretch of it that is in memory. No React, no timers: the
// controller (controller.ts) drives it, and the tests drive it directly.
import type { SlideOrder } from "../api";

/** What a slide needs to be found and drawn; every list entry of the API has at least this. */
export interface SlideItem {
  id: string;
  sort_time?: string;
  width?: number | null;
  height?: number | null;
  thumbhash?: string | null;
  thumb_url?: string | null;
  original_name?: string;
  archived?: boolean;
  /** An address of the preview that came with the item (a share link's own); otherwise one is asked for just in time. */
  preview_url?: string | null;
}

/** One chunk of a pass over the photos. */
export interface FeedPage {
  items: SlideItem[];
  /** No chunk follows this one. */
  done: boolean;
  /** Photos in the whole pass, when the source knows. */
  total?: number;
}

/** A pass over the photos, chunk by chunk. A `next()` that throws can be called again and asks for the same chunk. */
export interface Feed {
  next(): Promise<FeedPage>;
}

/** Where the photos come from: pages of an album, a search, the library... */
export interface SlideSource {
  /** Shown in the bar ("Holiday 2024"). */
  title: string;
  /** A new pass over the photos in `order`; `seed` fixes a shuffle, `pass` counts the passes (0 is the first). */
  open(order: SlideOrder, seed: string, pass: number): Feed;
  /** The preview address for a photo, when the source has its own way (a share link); the signed-in API's by default. */
  resolve?(item: SlideItem): Promise<{ url: string; caption?: string | null } | null>;
  /** What to say when the server answers "not signed in" and signing in is not the remedy (a share link's password). */
  expired?: { title: string; text: string };
}

// --- Seeded shuffle ----------------------------------------------------------------------------------------------------

/** A small, fast generator whose sequence is fixed by a text (xmur3 into mulberry32). */
export function seededRandom(seed: string): () => number {
  let h = 1779033703 ^ seed.length;
  for (let i = 0; i < seed.length; i++) {
    h = Math.imul(h ^ seed.charCodeAt(i), 3432918353);
    h = (h << 13) | (h >>> 19);
  }
  h = Math.imul(h ^ (h >>> 16), 2246822507);
  h = Math.imul(h ^ (h >>> 13), 3266489909);
  let a = (h ^ (h >>> 16)) >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** A random word to seed a shuffle with. */
export function newSeed(): string {
  const bytes = new Uint32Array(2);
  if (typeof crypto !== "undefined" && crypto.getRandomValues) crypto.getRandomValues(bytes);
  else bytes.forEach((_, i) => (bytes[i] = Math.floor(Math.random() * 4294967296)));
  return Array.from(bytes, (b) => b.toString(36)).join("");
}

/** Fisher-Yates over a copy: every order is equally likely, and the same seed gives the same one. */
export function shuffled<T>(items: readonly T[], seed: string): T[] {
  const out = items.slice();
  const rnd = seededRandom(seed);
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [out[i], out[j]] = [out[j]!, out[i]!];
  }
  return out;
}

/** Oldest first: by `sort_time`, then id (the server's own tie-break). */
export function oldestFirst<T extends SlideItem>(items: readonly T[]): T[] {
  return items.slice().sort((a, b) => (a.sort_time ?? "").localeCompare(b.sort_time ?? "") || a.id.localeCompare(b.id));
}

// --- The deck ----------------------------------------------------------------------------------------------------------

export interface DeckOptions {
  order: SlideOrder;
  /** Start a new pass (a new shuffle) when the last photo has been shown. */
  loop: boolean;
  /** A seed for each pass of a shuffle. */
  seed?: () => string;
  /** Photos behind the current one kept for "previous" (older ones are forgotten, so a show of days does not grow). */
  keep?: number;
  /** Photos this deck never shows (the frame leaves out archived ones). */
  skip?: (item: SlideItem) => boolean;
}

/**
 * The photos of a show: a window of the passes so far (`keep` behind the current photo, whatever has been fetched ahead),
 * filled one chunk at a time. `peek` asks for the photo in a direction without moving, `commit` moves onto it, so a photo
 * that could not be loaded is asked for again rather than skipped.
 *
 * Within one pass no photo comes twice; with `loop`, the last photo of a pass is followed by the first of the next one,
 * which is never the same photo (so a one-photo show does repeat it, and a shuffle never shows a photo twice in a row).
 */
export class SlideDeck {
  private buf: SlideItem[] = [];
  /** Index in `buf` of the current photo; -1 before the first. */
  private pos = -1;
  /** Where in `buf` each pass in memory began, and how many photos it has when the source said. */
  private starts: { at: number; total: number | null }[] = [{ at: 0, total: null }];
  private feed: Feed;
  private feedDone = false;
  private pass = 0;
  private seen = new Set<string>();
  private pending: Promise<void> | null = null;
  private opts: Required<Omit<DeckOptions, "skip">> & Pick<DeckOptions, "skip">;

  constructor(
    private source: SlideSource,
    opts: DeckOptions,
  ) {
    this.opts = { ...opts, seed: opts.seed ?? newSeed, keep: opts.keep ?? 60 };
    this.feed = this.openFeed();
  }

  private openFeed(): Feed {
    return this.source.open(this.opts.order, this.opts.order === "shuffle" ? this.opts.seed() : "", this.pass);
  }

  setLoop(loop: boolean) {
    this.opts.loop = loop;
  }

  /** The photo being shown. */
  get current(): SlideItem | null {
    return this.buf[this.pos] ?? null;
  }

  /** The pass the current photo belongs to (the chunk read ahead may already be the next pass's). */
  private get currentPass() {
    let found = this.starts[0]!;
    for (const s of this.starts) if (s.at <= this.pos) found = s;
    return found;
  }

  /** 1-based place of the current photo in its pass. */
  get position(): number {
    return this.pos < 0 ? 0 : this.pos - this.currentPass.at + 1;
  }

  /** Photos in the current pass, when the source said. */
  get total(): number | null {
    return this.currentPass.total;
  }

  /** Photos held in memory (for tests and the memory budget). */
  get held(): number {
    return this.buf.length;
  }

  /** Fetch one more chunk of the pass, or start the next pass. Returns false when there is nothing more. */
  private async more(): Promise<boolean> {
    if (this.pending) {
      await this.pending;
      return true;
    }
    if (this.feedDone) {
      if (!this.opts.loop || this.buf.length === 0) return false;
      this.pass += 1;
      this.feed = this.openFeed();
      this.feedDone = false;
      this.seen = new Set();
      this.starts.push({ at: this.buf.length, total: null });
    }
    const run = (async () => {
      const page = await this.feed.next();
      this.feedDone = page.done;
      const pass = this.starts[this.starts.length - 1]!;
      if (page.total !== undefined) pass.total = this.opts.skip ? null : page.total;
      let fresh = page.items.filter((i) => !this.seen.has(i.id) && !this.opts.skip?.(i));
      for (const i of fresh) this.seen.add(i.id);
      // The first photo of a new pass is not the last one of the pass before.
      const start = pass.at;
      if (this.pass > 0 && this.buf.length === start && fresh.length > 1 && fresh[0]!.id === this.buf[start - 1]?.id) {
        fresh = [fresh[1]!, fresh[0]!, ...fresh.slice(2)];
      }
      this.buf.push(...fresh);
    })();
    this.pending = run;
    try {
      await run;
    } finally {
      this.pending = null;
    }
    return true;
  }

  /** The photo after the current one, fetching as needed; null at the end (or for an empty set). */
  async peek(dir: 1 | -1): Promise<SlideItem | null> {
    if (dir < 0) return this.buf[this.pos - 1] ?? null;
    let guard = 0;
    while (this.pos + 1 >= this.buf.length) {
      const before = this.buf.length;
      if (!(await this.more())) return null;
      // A chunk of nothing new (every photo skipped or seen): ask again, but never forever.
      if (this.buf.length === before && this.feedDone && !this.opts.loop) return null;
      if (this.buf.length === before && ++guard > 50) return null;
    }
    return this.buf[this.pos + 1] ?? null;
  }

  /** Move onto the photo `peek(dir)` named. */
  commit(dir: 1 | -1) {
    const to = this.pos + dir;
    if (to < 0 || to >= this.buf.length) return;
    this.pos = to;
    // Forget the far past.
    const extra = this.pos - this.opts.keep;
    if (extra > 0) {
      this.buf.splice(0, extra);
      this.pos -= extra;
      for (const s of this.starts) s.at -= extra;
      while (this.starts.length > 1 && this.starts[1]!.at <= 0) this.starts.shift();
      if (this.starts[0]!.at < 0) this.starts[0]!.at = 0;
    }
  }

  /** Make sure `n` photos after the current one are in memory (page ahead while the current photo is on screen). */
  async fill(n: number): Promise<void> {
    let guard = 0;
    while (this.buf.length - 1 - this.pos < n && !(this.feedDone && !this.opts.loop) && guard++ < 50) {
      const before = this.buf.length;
      if (!(await this.more())) return;
      if (this.buf.length === before && this.feedDone) return;
    }
  }

  /** Photos after the current one that are already in memory. */
  upcoming(n: number): SlideItem[] {
    return this.buf.slice(this.pos + 1, this.pos + 1 + n);
  }
}
