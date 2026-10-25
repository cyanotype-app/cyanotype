// Helpers shared by the slideshow tests.
import { oldestFirst, shuffled, type Feed, type FeedPage, type SlideItem, type SlideSource } from "./deck";

/** `n` photos, `p1` the newest. */
export const photos = (n: number, prefix = "p"): SlideItem[] =>
  Array.from({ length: n }, (_, i) => ({ id: `${prefix}${i + 1}`, sort_time: `2024-01-${String(30 - i).padStart(2, "0")}T00:00:00Z` }));

/** A source that hands `items` out `size` at a time, counting the chunks asked for. */
export function chunked(items: SlideItem[], size: number) {
  const opened: { order: string; seed: string; pass: number }[] = [];
  const asked = { n: 0 };
  const source: SlideSource = {
    title: "t",
    open(order, seed, pass): Feed {
      opened.push({ order, seed, pass });
      let at = 0;
      return {
        async next(): Promise<FeedPage> {
          asked.n++;
          const ordered = order === "oldest" ? oldestFirst(items) : order === "shuffle" ? shuffled(items, seed) : items;
          const part = ordered.slice(at, at + size);
          at += size;
          return { items: part, done: at >= ordered.length, total: ordered.length };
        },
      };
    },
  };
  return { source, opened, asked };
}
