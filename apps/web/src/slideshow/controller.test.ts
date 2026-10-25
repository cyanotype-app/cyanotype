import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { SlideshowController, type ControllerOptions, type Loaded } from "./controller";
import type { SlideItem } from "./deck";
import { chunked, photos } from "./testKit";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

/** Loaded pictures whose release can be told apart. */
function loader() {
  const live = new Set<string>();
  let n = 0;
  const load = vi.fn(async (item: SlideItem): Promise<Loaded> => {
    const tag = `${item.id}#${n++}`;
    live.add(tag);
    const img = { removeAttribute: () => live.delete(tag) } as unknown as HTMLImageElement;
    return { item, url: `/p/${item.id}`, caption: null, img, width: 4, height: 3 };
  });
  return { load, live };
}

function make(over: Partial<ControllerOptions> & { items?: SlideItem[]; chunk?: number } = {}) {
  const l = loader();
  const { source, asked, opened } = chunked(over.items ?? photos(5), over.chunk ?? 100);
  const ctl = new SlideshowController({
    source,
    order: "listed",
    loop: false,
    durationMs: 5000,
    transition: "none",
    load: l.load,
    ...over,
  });
  return { ctl, ...l, asked, opened };
}

const settle = async (ms = 0) => {
  await vi.advanceTimersByTimeAsync(ms);
};
const ids = (ctl: SlideshowController) => ctl.getState().slide?.item.id;

describe("timing", () => {
  it("shows the first photo, then the next every `duration`", async () => {
    const { ctl } = make();
    ctl.start();
    await settle();
    expect(ids(ctl)).toBe("p1");
    expect(ctl.getState().status).toBe("playing");
    await settle(4999);
    expect(ids(ctl)).toBe("p1");
    await settle(1);
    expect(ids(ctl)).toBe("p2");
    await settle(5000);
    expect(ids(ctl)).toBe("p3");
    ctl.destroy();
  });

  it("the time counts from when a photo appears, not from when it was asked for", async () => {
    let gate: () => void = () => undefined;
    const slow = vi.fn(async (item: SlideItem): Promise<Loaded> => {
      if (item.id === "p2") await new Promise<void>((r) => (gate = r));
      return { item, url: item.id, caption: null, img: null, width: 1, height: 1 };
    });
    const { ctl } = make({ load: slow });
    ctl.start();
    await settle();
    await settle(5000); // due: p2 is still loading
    expect(ids(ctl)).toBe("p1");
    await settle(20_000);
    expect(ids(ctl)).toBe("p1");
    gate();
    await settle();
    expect(ids(ctl)).toBe("p2");
    await settle(4999);
    expect(ids(ctl)).toBe("p2");
    await settle(1);
    expect(ids(ctl)).toBe("p3");
    ctl.destroy();
  });

  it("pause stops the clock and play starts a full interval again", async () => {
    const { ctl } = make();
    ctl.start();
    await settle();
    await settle(3000);
    ctl.pause();
    expect(ctl.getState().status).toBe("paused");
    expect(ctl.getState().announce).toBe("Slideshow paused");
    await settle(60_000);
    expect(ids(ctl)).toBe("p1");
    ctl.play();
    expect(ctl.getState().status).toBe("playing");
    await settle(4999);
    expect(ids(ctl)).toBe("p1");
    await settle(1);
    expect(ids(ctl)).toBe("p2");
    ctl.destroy();
  });

  it("next and previous work while paused and keep it paused", async () => {
    const { ctl } = make();
    ctl.start();
    await settle();
    ctl.pause();
    ctl.next();
    await settle();
    expect(ids(ctl)).toBe("p2");
    ctl.next();
    await settle();
    ctl.prev();
    await settle();
    expect(ids(ctl)).toBe("p2");
    expect(ctl.getState().status).toBe("paused");
    await settle(60_000);
    expect(ids(ctl)).toBe("p2");
    ctl.destroy();
  });

  it("a manual move restarts the interval", async () => {
    const { ctl } = make();
    ctl.start();
    await settle();
    await settle(4000);
    ctl.next();
    await settle();
    expect(ids(ctl)).toBe("p2");
    await settle(4999);
    expect(ids(ctl)).toBe("p2");
    await settle(1);
    expect(ids(ctl)).toBe("p3");
    ctl.destroy();
  });

  it("changing the duration while playing takes effect at once", async () => {
    const { ctl } = make();
    ctl.start();
    await settle();
    ctl.setOptions({ durationMs: 1000 });
    await settle(1000);
    expect(ids(ctl)).toBe("p2");
    ctl.destroy();
  });

  it("previous at the first photo stays put and says so", async () => {
    const { ctl } = make();
    ctl.start();
    await settle();
    ctl.prev();
    await settle();
    expect(ids(ctl)).toBe("p1");
    expect(ctl.getState().announce).toBe("This is the first photo");
    await settle(5000);
    expect(ids(ctl)).toBe("p2");
    ctl.destroy();
  });
});

describe("ends, loops and order", () => {
  it("stops after the last photo without a loop, and plays again on request", async () => {
    const { ctl } = make({ items: photos(2) });
    ctl.start();
    await settle();
    await settle(5000);
    expect(ids(ctl)).toBe("p2");
    await settle(5000);
    expect(ctl.getState().status).toBe("ended");
    expect(ctl.getState().announce).toBe("End of the slideshow");
    expect(ids(ctl)).toBe("p2");
    await settle(60_000);
    expect(ctl.getState().status).toBe("ended");
    ctl.play();
    await settle();
    expect(ids(ctl)).toBe("p1");
    expect(ctl.getState().status).toBe("playing");
    ctl.destroy();
  });

  it("goes round again with a loop", async () => {
    const { ctl } = make({ items: photos(3), loop: true });
    ctl.start();
    await settle();
    const seen = [ids(ctl)];
    for (let i = 0; i < 7; i++) {
      await settle(5000);
      seen.push(ids(ctl));
    }
    expect(seen).toEqual(["p1", "p2", "p3", "p1", "p2", "p3", "p1", "p2"]);
    ctl.destroy();
  });

  it("an empty set says so", async () => {
    const { ctl } = make({ items: [] });
    ctl.start();
    await settle();
    expect(ctl.getState().status).toBe("empty");
    expect(ctl.getState().slide).toBeNull();
    ctl.destroy();
  });

  it("shuffle shows every photo once per cycle, and a new cycle is a new order", async () => {
    const items = photos(12);
    const { ctl } = make({ items, order: "shuffle", loop: true, chunk: 5, durationMs: 1000 });
    ctl.start();
    await settle();
    const seen = [ids(ctl)!];
    for (let i = 1; i < 36; i++) {
      await settle(1000);
      seen.push(ids(ctl)!);
    }
    const cycles = [seen.slice(0, 12), seen.slice(12, 24), seen.slice(24)];
    for (const c of cycles) expect(new Set(c).size).toBe(12);
    expect(cycles[0]).not.toEqual(cycles[1]);
    expect(cycles[0]).not.toEqual(items.map((i) => i.id));
    ctl.destroy();
  });

  it("changing the order starts over in that order", async () => {
    const { ctl } = make({ items: photos(4) });
    ctl.start();
    await settle();
    await settle(5000);
    expect(ids(ctl)).toBe("p2");
    ctl.setOptions({ order: "oldest" });
    await settle();
    // Oldest first: p4 (the latest p-number has the earliest date).
    expect(ids(ctl)).toBe("p4");
    ctl.destroy();
  });

  it("reads the source a chunk at a time, ahead of need", async () => {
    const { ctl, asked } = make({ items: photos(30), chunk: 10, durationMs: 1000 });
    ctl.start();
    await settle();
    expect(asked.n).toBeLessThanOrEqual(1);
    for (let i = 0; i < 6; i++) await settle(1000);
    expect(asked.n).toBeLessThanOrEqual(2);
    for (let i = 0; i < 6; i++) await settle(1000);
    expect(asked.n).toBeLessThanOrEqual(3);
    ctl.destroy();
  });
});

describe("transitions", () => {
  it("crossfades: the old photo stays underneath, then goes", async () => {
    const { ctl, live } = make({ transition: "crossfade", fadeMs: 800 });
    ctl.start();
    await settle();
    expect(ctl.getState().outgoing).toBeNull();
    await settle(5000);
    expect(ids(ctl)).toBe("p2");
    expect(ctl.getState().outgoing?.item.id).toBe("p1");
    expect(ctl.getState().fade).toBe(true);
    await settle(800);
    expect(ctl.getState().outgoing).toBeNull();
    // Nothing of p1 is kept.
    expect([...live].some((t) => t.startsWith("p1#"))).toBe(false);
    ctl.destroy();
  });

  it("respects reduced motion: cut, no layer left over", async () => {
    const { ctl } = make({ transition: "crossfade", reducedMotion: () => true });
    ctl.start();
    await settle();
    await settle(5000);
    expect(ids(ctl)).toBe("p2");
    expect(ctl.getState().outgoing).toBeNull();
    expect(ctl.getState().fade).toBe(false);
    ctl.destroy();
  });

  it("\"None\" is a cut as well", async () => {
    const { ctl } = make({ transition: "none" });
    ctl.start();
    await settle();
    ctl.next();
    await settle();
    expect(ctl.getState().outgoing).toBeNull();
    ctl.destroy();
  });
});

describe("announcements", () => {
  it("says nothing for a photo that changed by itself, and the place for a manual move", async () => {
    const { ctl } = make({ items: photos(4) });
    ctl.start();
    await settle();
    await settle(5000);
    expect(ctl.getState().announce).toBe("");
    ctl.next();
    await settle();
    expect(ctl.getState().announce).toBe("Photo 3 of 4");
    ctl.pause();
    expect(ctl.getState().announce).toBe("Slideshow paused");
    ctl.play();
    expect(ctl.getState().announce).toBe("Slideshow playing");
    ctl.destroy();
  });
});

describe("memory", () => {
  it("lets go of every picture it is no longer showing or about to show", async () => {
    const { ctl, live } = make({ items: photos(40), loop: true, order: "shuffle", chunk: 8, durationMs: 1000, transition: "crossfade", fadeMs: 300 });
    ctl.start();
    await settle();
    for (let i = 0; i < 300; i++) {
      await settle(1000);
      // current, a fading one, and the preloaded next.
      expect(live.size).toBeLessThanOrEqual(3);
    }
    ctl.destroy();
    await settle();
    expect(live.size).toBe(0);
  });

  it("drops the preloaded photo when the user goes the other way", async () => {
    const { ctl, live } = make({ items: photos(10) });
    ctl.start();
    await settle();
    ctl.next();
    await settle();
    ctl.prev();
    await settle();
    expect(live.size).toBeLessThanOrEqual(2);
    ctl.destroy();
    await settle();
    expect(live.size).toBe(0);
  });
});

describe("trouble", () => {
  it("keeps the photo on screen while the server is out of reach, retries with a growing wait, and carries on", async () => {
    let down = false;
    const l = loader();
    const load = vi.fn(async (item: SlideItem) => {
      if (down) throw new ApiError(0, "Cannot reach the server.");
      return l.load(item);
    });
    const { ctl } = make({ load, retryDelays: [1000, 2000, 4000], durationMs: 5000 });
    ctl.start();
    await settle();
    down = true;
    await settle(5000); // p2 was preloaded before the outage: shown
    expect(ids(ctl)).toBe("p2");
    await settle(5000); // p3 cannot be loaded
    expect(ids(ctl)).toBe("p2");
    expect(ctl.getState().trouble).toBe(true);
    const calls = load.mock.calls.length;
    await settle(1000);
    expect(load.mock.calls.length).toBe(calls + 1);
    await settle(1999);
    expect(load.mock.calls.length).toBe(calls + 1);
    await settle(1);
    expect(load.mock.calls.length).toBe(calls + 2);
    await settle(4000); // the wait stops growing at the last value
    await settle(4000);
    expect(ids(ctl)).toBe("p2");
    down = false;
    await settle(4000);
    expect(ids(ctl)).toBe("p3");
    expect(ctl.getState().trouble).toBe(false);
    await settle(5000);
    expect(ids(ctl)).toBe("p4");
    ctl.destroy();
  });

  it("tries again at once when told the network is back", async () => {
    let down = true;
    const l = loader();
    const load = vi.fn(async (item: SlideItem) => {
      if (down) throw new ApiError(0, "Cannot reach the server.");
      return l.load(item);
    });
    const { ctl } = make({ load, retryDelays: [60_000] });
    ctl.start();
    await settle();
    expect(ctl.getState().trouble).toBe(true);
    expect(ids(ctl)).toBeUndefined();
    down = false;
    ctl.retryNow();
    await settle();
    expect(ids(ctl)).toBe("p1");
    expect(ctl.getState().trouble).toBe(false);
    ctl.destroy();
  });

  it("a session that ended is not retried: it says so", async () => {
    const l = loader();
    let dead = false;
    const load = vi.fn(async (item: SlideItem) => {
      if (dead) throw new ApiError(401, "Not signed in");
      return l.load(item);
    });
    const { ctl } = make({ load });
    ctl.start();
    await settle();
    dead = true;
    await settle(10_000);
    expect(ctl.getState().status).toBe("signed-out");
    const calls = load.mock.calls.length;
    await settle(10 * 60_000);
    expect(load.mock.calls.length).toBe(calls);
    ctl.destroy();
  });

  it("a request that never answers counts as a failure, and is tried again", async () => {
    let hangId: string | null = null;
    const l = loader();
    const load = vi.fn((item: SlideItem) => (item.id === hangId ? new Promise<Loaded>(() => undefined) : l.load(item)));
    const { ctl } = make({ load, moveTimeoutMs: 10_000, retryDelays: [1000], items: photos(6) });
    ctl.start();
    await settle();
    ctl.next();
    await settle();
    hangId = "p4";
    ctl.next(); // p3 (preloaded); p4 is now being preloaded, and never arrives
    await settle();
    expect(ids(ctl)).toBe("p3");
    ctl.next();
    await settle(9999);
    expect(ctl.getState().trouble).toBe(false);
    await settle(1);
    expect(ctl.getState().trouble).toBe(true);
    expect(ids(ctl)).toBe("p3");
    hangId = null;
    await settle(1000);
    expect(ids(ctl)).toBe("p4");
    expect(ctl.getState().trouble).toBe(false);
    ctl.destroy();
  });

  it("a photo that will not load is skipped; photos that all fail are trouble, not a tour of the whole set", async () => {
    const l = loader();
    const load = vi.fn(async (item: SlideItem) => (item.id === "p2" ? null : l.load(item)));
    const { ctl } = make({ load });
    ctl.start();
    await settle();
    ctl.next();
    await settle();
    expect(ids(ctl)).toBe("p3");
    ctl.destroy();

    const none = make({ load: vi.fn(async () => null), items: photos(50), retryDelays: [60_000] });
    none.ctl.start();
    await settle();
    expect(none.ctl.getState().trouble).toBe(true);
    expect(none.load.mock.calls.length).toBeLessThanOrEqual(6);
    none.ctl.destroy();
  });

  it("offline, a photo that fails is not skipped", async () => {
    let offline = true;
    const l = loader();
    const load = vi.fn(async (item: SlideItem) => (offline ? null : l.load(item)));
    const { ctl } = make({ load, offline: () => offline, retryDelays: [1000] });
    ctl.start();
    await settle();
    expect(ctl.getState().trouble).toBe(true);
    offline = false;
    await settle(1000);
    expect(ids(ctl)).toBe("p1");
    ctl.destroy();
  });

  it("a user move supersedes a move in flight, whose photo is dropped", async () => {
    const pending: (() => void)[] = [];
    const l = loader();
    const load = vi.fn(
      (item: SlideItem) =>
        new Promise<Loaded>((resolve) => {
          pending.push(() => void l.load(item).then(resolve));
        }),
    );
    const { ctl, live } = make({ load });
    ctl.start();
    await settle();
    pending.shift()!(); // p1 shown
    await settle();
    expect(ids(ctl)).toBe("p1");
    await settle(5000); // auto move to p2 starts (waits on the loader)
    ctl.prev(); // at the first photo: nothing to load, the auto move is cancelled? it was already loading p2
    await settle();
    while (pending.length) pending.shift()!();
    await settle();
    // Whatever was loading in the background is not left behind.
    expect(live.size).toBeLessThanOrEqual(2);
    ctl.destroy();
    await settle();
    expect(live.size).toBe(0);
  });
});
