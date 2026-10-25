// The slideshow's state machine: which photo is on screen, when the next one is due, what happens when a photo cannot be
// loaded or the server cannot be reached. Plain TypeScript (timers and the loader are injected), so the tests run it with fake
// timers; the React components only draw its state and forward clicks and keys.
//
// One move at a time: every move (auto, next, previous, a retry) takes a token and a newer move cancels an older one, whose
// photo is then dropped. A photo is shown only once it has been decoded, so there is never a half-drawn picture or a flash.
// The photo after the current one is loaded while the current one is on screen.
import { ApiError, type SlideOrder } from "../api";
import { SlideDeck, type SlideItem, type SlideSource } from "./deck";

/** A photo that is ready to be drawn. */
export interface Loaded {
  item: SlideItem;
  /** The preview's address. */
  url: string;
  caption: string | null;
  /** The decoded image, kept until the photo has been shown so the browser does not let go of it. */
  img: HTMLImageElement | null;
  width: number | null;
  height: number | null;
}

export type ShowStatus = "loading" | "playing" | "paused" | "ended" | "empty" | "signed-out";

export interface ShowState {
  status: ShowStatus;
  slide: Loaded | null;
  /** The photo that is fading out, while a crossfade runs (otherwise null: at most two photos exist at once). */
  outgoing: Loaded | null;
  /** 1-based place in the pass, 0 before the first photo. */
  position: number;
  /** Photos in the pass, when known. */
  total: number | null;
  /** The server cannot be reached (or photos keep failing): the current photo stays and a retry is scheduled. */
  trouble: boolean;
  /** What a screen reader should say about the last change; empty for a slide that changed by itself. */
  announce: string;
  /** Whether the change that put `slide` there fades (false under reduced motion or "None"). */
  fade: boolean;
}

export interface ControllerOptions {
  source: SlideSource;
  order: SlideOrder;
  loop: boolean;
  durationMs: number;
  transition: "crossfade" | "none";
  /** Resolve a photo's preview and decode it; null when it has none to show or will not load. Throws when the API fails. */
  load: (item: SlideItem, ahead: SlideItem[]) => Promise<Loaded | null>;
  reducedMotion?: () => boolean;
  /** True while the browser knows it is offline: a photo that fails then is not skipped, it is tried again later. */
  offline?: () => boolean;
  skip?: (item: SlideItem) => boolean;
  seed?: () => string;
  /** Waits (ms) before each retry after a failure; the last one repeats. */
  retryDelays?: number[];
  fadeMs?: number;
  /** A move that has not finished after this long (a request that hangs on a dropped connection) counts as a failure. */
  moveTimeoutMs?: number;
  /** Photos kept ready ahead of the current one (their list entries; one preview is preloaded). */
  lookahead?: number;
  autoplay?: boolean;
}

export const DEFAULT_MOVE_TIMEOUT_MS = 45_000;
export const DEFAULT_RETRY_DELAYS = [2000, 5000, 15000, 30000, 60000, 300000];
/** This many photos in a row that will not load count as trouble (a lost connection or an expired link), not as bad photos. */
const MAX_SKIPS = 3;

export function releaseSlide(l: Loaded | null) {
  // Let go of the decoded picture: a show of days must not keep what it has shown.
  l?.img?.removeAttribute("src");
}

export class SlideshowController {
  private state: ShowState = {
    status: "loading",
    slide: null,
    outgoing: null,
    position: 0,
    total: null,
    trouble: false,
    announce: "",
    fade: false,
  };
  private listeners = new Set<() => void>();
  private deck: SlideDeck;
  private opts: ControllerOptions;
  private playing: boolean;
  private moving = false;
  private lastDir: 1 | -1 = 1;
  private token = 0;
  private attempts = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private fadeTimer: ReturnType<typeof setTimeout> | null = null;
  private prefetch: { id: string; promise: Promise<Loaded | null> } | null = null;
  private destroyed = false;

  constructor(opts: ControllerOptions) {
    this.opts = opts;
    this.playing = opts.autoplay ?? true;
    this.deck = this.newDeck();
  }

  private newDeck(): SlideDeck {
    return new SlideDeck(this.opts.source, {
      order: this.opts.order,
      loop: this.opts.loop,
      seed: this.opts.seed,
      skip: this.opts.skip,
    });
  }

  // --- Observing ---------------------------------------------------------------------------------------------------

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  getState = (): ShowState => this.state;

  private set(patch: Partial<ShowState>) {
    this.state = { ...this.state, ...patch };
    for (const fn of [...this.listeners]) fn();
  }

  // --- Commands ----------------------------------------------------------------------------------------------------

  start() {
    void this.move(1, "auto");
  }

  next() {
    void this.move(1, "user");
  }

  prev() {
    void this.move(-1, "user");
  }

  get isPlaying(): boolean {
    return this.playing;
  }

  toggle() {
    if (this.playing) this.pause();
    else this.play();
  }

  play() {
    if (this.destroyed) return;
    if (this.state.status === "ended") {
      this.playing = true;
      this.restart();
      return;
    }
    this.playing = true;
    if (this.state.slide) {
      this.set({ status: "playing", announce: "Slideshow playing" });
      if (!this.moving && !this.retryTimer) this.armTimer();
    }
  }

  pause() {
    if (this.destroyed) return;
    this.playing = false;
    this.clearTimers();
    // A move that is still loading is dropped; the current photo stays.
    if (this.moving) {
      this.token++;
      this.moving = false;
    }
    if (this.state.slide) this.set({ status: "paused", announce: "Slideshow paused" });
  }

  /** Begin again from the first photo of a new pass (after the end, or a new order). */
  restart() {
    if (this.destroyed) return;
    this.token++;
    this.moving = false;
    this.clearTimers();
    this.dropPrefetch();
    this.deck = this.newDeck();
    this.attempts = 0;
    this.set({ trouble: false, status: this.state.slide ? (this.playing ? "playing" : "paused") : "loading" });
    void this.move(1, "user");
  }

  setOptions(patch: Partial<Pick<ControllerOptions, "order" | "loop" | "durationMs" | "transition">>) {
    const orderChanged = patch.order !== undefined && patch.order !== this.opts.order;
    this.opts = { ...this.opts, ...patch };
    this.deck.setLoop(this.opts.loop);
    if (orderChanged) {
      this.restart();
    } else if (patch.durationMs !== undefined && this.playing && this.timer) {
      this.armTimer();
    }
  }

  /** The browser came back online (or the user asked): try the failed move again now. */
  retryNow() {
    if (this.destroyed || !this.state.trouble) return;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    void this.move(this.lastDir, "retry");
  }

  destroy() {
    this.destroyed = true;
    this.token++;
    this.clearTimers();
    this.dropPrefetch();
    releaseSlide(this.state.slide);
    releaseSlide(this.state.outgoing);
    this.state = { ...this.state, slide: null, outgoing: null };
    this.listeners.clear();
  }

  // --- Moving ------------------------------------------------------------------------------------------------------

  private fadeOn(): boolean {
    return this.opts.transition === "crossfade" && !this.opts.reducedMotion?.();
  }

  private clearTimers() {
    if (this.timer) clearTimeout(this.timer);
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.timer = this.retryTimer = null;
  }

  private armTimer() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.move(1, "auto");
    }, this.opts.durationMs);
  }

  private dropPrefetch() {
    const p = this.prefetch;
    this.prefetch = null;
    p?.promise.then(releaseSlide, () => undefined);
  }

  private async take(item: SlideItem): Promise<Loaded | null> {
    const p = this.prefetch;
    if (p && p.id === item.id) {
      this.prefetch = null;
      return p.promise;
    }
    this.dropPrefetch();
    return this.opts.load(item, this.deck.upcoming(this.opts.lookahead ?? 3));
  }

  private async move(dir: 1 | -1, cause: "auto" | "user" | "retry") {
    if (this.destroyed) return;
    const token = ++this.token;
    this.moving = true;
    this.lastDir = dir;
    this.clearTimers();
    // A request that never answers must not freeze the show for good.
    const stall = setTimeout(() => {
      if (token !== this.token) return;
      this.token++;
      this.moving = false;
      this.fail(new Error("timed out"), dir);
    }, this.opts.moveTimeoutMs ?? DEFAULT_MOVE_TIMEOUT_MS);
    try {
      let skips = 0;
      for (;;) {
        const item = await this.deck.peek(dir);
        if (token !== this.token) return;
        if (!item) {
          this.moving = false;
          this.edge(dir, cause);
          return;
        }
        const loaded = dir > 0 ? await this.take(item) : await this.opts.load(item, []);
        if (token !== this.token) {
          releaseSlide(loaded);
          return;
        }
        if (loaded) {
          this.deck.commit(dir);
          this.show(loaded, cause);
          return;
        }
        if (this.opts.offline?.() || ++skips >= MAX_SKIPS) throw new Error("photos will not load");
        this.deck.commit(dir);
      }
    } catch (e) {
      if (token !== this.token) return;
      this.moving = false;
      this.fail(e, dir);
    } finally {
      clearTimeout(stall);
    }
  }

  private show(loaded: Loaded, cause: "auto" | "user" | "retry") {
    this.moving = false;
    this.attempts = 0;
    const fade = this.fadeOn();
    if (this.fadeTimer) clearTimeout(this.fadeTimer);
    this.fadeTimer = null;
    releaseSlide(this.state.outgoing);
    const prev = this.state.slide;
    let outgoing: Loaded | null = null;
    if (prev) {
      if (fade) outgoing = prev;
      else releaseSlide(prev);
    }
    const position = this.deck.position;
    const total = this.deck.total;
    this.set({
      status: this.playing ? "playing" : "paused",
      slide: loaded,
      outgoing,
      position,
      total,
      trouble: false,
      fade,
      announce: cause === "user" ? `Photo ${position}${total ? ` of ${total}` : ""}` : "",
    });
    if (outgoing) {
      this.fadeTimer = setTimeout(() => {
        this.fadeTimer = null;
        releaseSlide(outgoing);
        this.set({ outgoing: null });
      }, this.opts.fadeMs ?? 900);
    }
    if (this.playing) this.armTimer();
    this.lookAhead(this.token);
  }

  /** While the current photo is on screen: page the source one chunk ahead if needed and load the next photo. */
  private lookAhead(token: number) {
    void (async () => {
      try {
        await this.deck.fill(this.opts.lookahead ?? 3);
        if (token !== this.token) return;
        const item = await this.deck.peek(1);
        if (!item || token !== this.token || this.prefetch?.id === item.id) return;
        this.dropPrefetch();
        const promise = this.opts.load(item, this.deck.upcoming(this.opts.lookahead ?? 3));
        promise.catch(() => undefined);
        this.prefetch = { id: item.id, promise };
      } catch {
        /* the move itself asks again, and reports the failure */
      }
    })();
  }

  private edge(dir: 1 | -1, cause: "auto" | "user" | "retry") {
    if (dir > 0) {
      this.playing = false;
      this.set(
        this.state.slide
          ? { status: "ended", announce: "End of the slideshow" }
          : { status: "empty", announce: "There are no photos to show" },
      );
      return;
    }
    // Already at the first photo: stay, and keep the show going.
    if (cause === "user") this.set({ announce: "This is the first photo" });
    if (this.playing) this.armTimer();
  }

  private fail(e: unknown, dir: 1 | -1) {
    if (e instanceof ApiError && e.status === 401) {
      this.playing = false;
      this.clearTimers();
      this.set({ status: "signed-out", trouble: false, announce: "Your session has ended. Sign in again." });
      return;
    }
    const delays = this.opts.retryDelays ?? DEFAULT_RETRY_DELAYS;
    const delay = delays[Math.min(this.attempts, delays.length - 1)] ?? 5000;
    this.attempts++;
    this.set({ trouble: true, announce: this.state.trouble ? this.state.announce : "Cannot reach the server. Trying again." });
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.move(dir, "retry");
    }, delay);
  }
}
