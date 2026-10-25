import { describe, expect, it, vi } from "vitest";
import { SlideDeck, oldestFirst, seededRandom, shuffled, type Feed, type SlideSource } from "./deck";
import { chunked, photos } from "./testKit";

async function run(deck: SlideDeck, n: number): Promise<string[]> {
  const out: string[] = [];
  for (let i = 0; i < n; i++) {
    const item = await deck.peek(1);
    if (!item) break;
    deck.commit(1);
    out.push(item.id);
  }
  return out;
}

describe("seeded shuffle", () => {
  it("is the same for the same seed, different for another, and a permutation", () => {
    const items = photos(50);
    const a = shuffled(items, "alpha");
    expect(shuffled(items, "alpha")).toEqual(a);
    expect(shuffled(items, "beta")).not.toEqual(a);
    expect(a.map((i) => i.id).sort()).toEqual(items.map((i) => i.id).sort());
    expect(a).not.toEqual(items);
    // The input is left alone.
    expect(items[0]!.id).toBe("p1");
  });

  it("is fair: every photo is equally likely at every place", () => {
    const n = 5;
    const runs = 4000;
    const count = Array.from({ length: n }, () => new Array<number>(n).fill(0));
    const base = photos(n);
    for (let s = 0; s < runs; s++) {
      shuffled(base, `seed-${s}`).forEach((it, place) => count[Number(it.id.slice(1)) - 1]![place]!++);
    }
    const expected = runs / n;
    for (const row of count) for (const c of row) expect(Math.abs(c - expected) / expected).toBeLessThan(0.12);
  });

  it("random numbers stay in [0, 1) and repeat per seed", () => {
    const r = seededRandom("x");
    const first = Array.from({ length: 5 }, r);
    expect(first.every((v) => v >= 0 && v < 1)).toBe(true);
    const again = seededRandom("x");
    expect(Array.from({ length: 5 }, again)).toEqual(first);
  });

  it("oldest first sorts by time, then id", () => {
    const items = [
      { id: "b", sort_time: "2024-01-02T00:00:00Z" },
      { id: "a", sort_time: "2024-01-02T00:00:00Z" },
      { id: "c", sort_time: "2023-01-01T00:00:00Z" },
    ];
    expect(oldestFirst(items).map((i) => i.id)).toEqual(["c", "a", "b"]);
  });
});

describe("SlideDeck", () => {
  it("pages lazily: a chunk is read only when the show gets near its end", async () => {
    const { source, asked } = chunked(photos(100), 10);
    const deck = new SlideDeck(source, { order: "listed", loop: false });
    expect(asked.n).toBe(0);
    await run(deck, 3);
    expect(asked.n).toBe(1);
    await deck.fill(3);
    expect(asked.n).toBe(1);
    await run(deck, 7); // 10 shown
    await deck.fill(3);
    expect(asked.n).toBe(2);
    expect(deck.total).toBe(100);
  });

  it("shows every photo once in a pass and then ends without a loop", async () => {
    const { source } = chunked(photos(25), 10);
    const deck = new SlideDeck(source, { order: "shuffle", loop: false, seed: () => "s" });
    const ids = await run(deck, 100);
    expect(ids).toHaveLength(25);
    expect(new Set(ids).size).toBe(25);
    expect(await deck.peek(1)).toBeNull();
  });

  it("drops a photo a chunk repeats within the pass", async () => {
    const dup: Feed[] = [];
    const items = photos(4);
    const source: SlideSource = {
      title: "t",
      open() {
        const pages = [items.slice(0, 3), items.slice(2)];
        let i = 0;
        const f: Feed = {
          async next() {
            const p = pages[i++]!;
            return { items: p, done: i >= pages.length };
          },
        };
        dup.push(f);
        return f;
      },
    };
    const deck = new SlideDeck(source, { order: "listed", loop: false });
    expect(await run(deck, 10)).toEqual(["p1", "p2", "p3", "p4"]);
  });

  it("with a loop starts a fresh shuffle for every pass and never shows a photo twice in a row", async () => {
    const { source, opened } = chunked(photos(6), 4);
    let n = 0;
    const deck = new SlideDeck(source, { order: "shuffle", loop: true, seed: () => `seed${n++}` });
    const ids = await run(deck, 6 * 40);
    expect(opened.map((o) => o.seed).slice(0, 3)).toEqual(["seed0", "seed1", "seed2"]);
    expect(opened.map((o) => o.pass).slice(0, 3)).toEqual([0, 1, 2]);
    for (let p = 0; p < 40; p++) {
      const pass = ids.slice(p * 6, p * 6 + 6);
      expect(new Set(pass).size, `pass ${p} has no repeats`).toBe(6);
    }
    for (let i = 1; i < ids.length; i++) expect(ids[i]).not.toBe(ids[i - 1]);
  });

  it("the place and total belong to the pass on screen, even when the next pass has been read ahead", async () => {
    const { source } = chunked(photos(3), 10);
    const deck = new SlideDeck(source, { order: "listed", loop: true });
    await run(deck, 1);
    await deck.fill(8); // reads two more passes
    expect(deck.position).toBe(1);
    expect(deck.total).toBe(3);
    await run(deck, 3);
    expect(deck.current?.id).toBe("p1");
    expect(deck.position).toBe(1);
  });

  it("an empty set ends at once, with or without a loop", async () => {
    for (const loop of [false, true]) {
      const { source } = chunked([], 10);
      const deck = new SlideDeck(source, { order: "listed", loop });
      expect(await deck.peek(1)).toBeNull();
    }
  });

  it("goes back through what was shown", async () => {
    const { source } = chunked(photos(8), 4);
    const deck = new SlideDeck(source, { order: "listed", loop: false });
    await run(deck, 5);
    expect(deck.current?.id).toBe("p5");
    expect((await deck.peek(-1))?.id).toBe("p4");
    deck.commit(-1);
    deck.commit(-1);
    expect(deck.current?.id).toBe("p3");
    expect(deck.position).toBe(3);
    expect((await deck.peek(1))?.id).toBe("p4");
    // The start has nothing before it.
    deck.commit(-1);
    deck.commit(-1);
    expect(await deck.peek(-1)).toBeNull();
  });

  it("forgets the far past: a show of thousands of photos holds a bounded window", async () => {
    const { source } = chunked(photos(50), 10);
    const deck = new SlideDeck(source, { order: "shuffle", loop: true, keep: 20 });
    let most = 0;
    for (let i = 0; i < 2000; i++) {
      const it = await deck.peek(1);
      expect(it).not.toBeNull();
      deck.commit(1);
      most = Math.max(most, deck.held);
    }
    expect(most).toBeLessThanOrEqual(20 + 1 + 10 + 10);
    // And "previous" still works for the recent past.
    expect(await deck.peek(-1)).not.toBeNull();
  });

  it("leaves out photos it is told to skip", async () => {
    const items = photos(10).map((i, k) => ({ ...i, archived: k % 2 === 0 }));
    const { source } = chunked(items, 4);
    const deck = new SlideDeck(source, { order: "listed", loop: false, skip: (i) => i.archived === true });
    expect(await run(deck, 20)).toEqual(["p2", "p4", "p6", "p8", "p10"]);
    expect(deck.total).toBeNull();
  });

  it("a failed chunk is asked for again, not skipped", async () => {
    let fail = true;
    const source: SlideSource = {
      title: "t",
      open() {
        return {
          async next() {
            if (fail) {
              fail = false;
              throw new Error("down");
            }
            return { items: photos(2), done: true };
          },
        };
      },
    };
    const deck = new SlideDeck(source, { order: "listed", loop: false });
    await expect(deck.peek(1)).rejects.toThrow("down");
    expect(await run(deck, 5)).toEqual(["p1", "p2"]);
  });

  it("does not loop forever over a set where everything is skipped", async () => {
    const { source, asked } = chunked(photos(5), 5);
    const deck = new SlideDeck(source, { order: "listed", loop: true, skip: () => true });
    expect(await deck.peek(1)).toBeNull();
    expect(asked.n).toBeLessThan(200);
  });

  it("keeps one chunk request in flight at a time", async () => {
    const gate = vi.fn();
    let release: () => void = () => undefined;
    const source: SlideSource = {
      title: "t",
      open() {
        return {
          async next() {
            gate();
            await new Promise<void>((r) => (release = r));
            return { items: photos(3), done: true };
          },
        };
      },
    };
    const deck = new SlideDeck(source, { order: "listed", loop: false });
    const a = deck.peek(1);
    const b = deck.fill(2);
    await Promise.resolve();
    release();
    await Promise.all([a, b]);
    expect(gate).toHaveBeenCalledTimes(1);
  });
});
