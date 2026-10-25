// Where a slideshow's photos come from. A bulk-action scope (a whole album, the library...) is ordered and paged by the
// server, so a shuffle of a 50,000-photo library is fair and fetches 40 photos at a time. Anything else (a ranked search, the
// photos a page has loaded, a share link) is paged through its own list, and ordered here.
import { api, type Scope, type SearchParams, type SlideOrder } from "../api";
import { oldestFirst, shuffled, type Feed, type SlideItem, type SlideSource } from "./deck";

/** Photos fetched at a time from the server's ordered feed. */
export const SCOPE_PAGE = 40;
/** The most photos read from a list to order them here (oldest first and shuffle need all of them). */
export const MAX_LOCAL = 5000;

/** A bulk-action scope: the server pages it in the order asked for. */
export function scopeSource(scope: Scope, title: string): SlideSource {
  return {
    title,
    open(order, seed): Feed {
      let cursor: string | null = null;
      let done = false;
      return {
        async next() {
          if (done) return { items: [], done: true };
          const page = await api.slideshowPage(scope, { order, seed, cursor, limit: SCOPE_PAGE });
          cursor = page.next_cursor;
          done = !page.next_cursor;
          return { items: page.items, done, total: page.total };
        },
      };
    },
  };
}

/**
 * A list read page by page with a keyset cursor (a ranked search, a share link). As listed, it is read lazily, a page
 * ahead of the show; oldest first and shuffle read it all (up to `MAX_LOCAL` photos) and order it here.
 */
export function pagedSource(
  fetchPage: (cursor: string | null) => Promise<{ items: SlideItem[]; next_cursor?: string | null }>,
  title: string,
  opts: { startId?: string; resolve?: SlideSource["resolve"] } = {},
): SlideSource {
  return {
    title,
    resolve: opts.resolve,
    open(order, seed, pass): Feed {
      let cursor: string | null = null;
      let done = false;
      if (order === "listed" && !opts.startId) {
        return {
          async next() {
            if (done) return { items: [], done: true };
            const page = await fetchPage(cursor);
            cursor = page.next_cursor ?? null;
            done = !cursor;
            return { items: page.items, done };
          },
        };
      }
      // Read it all once, then hand it over in one chunk. A failed read starts again from where it stopped.
      let all: SlideItem[] = [];
      let handed = false;
      return {
        async next() {
          if (handed) return { items: [], done: true };
          while (!done && all.length < MAX_LOCAL) {
            const page = await fetchPage(cursor);
            all = all.concat(page.items);
            cursor = page.next_cursor ?? null;
            done = !cursor;
          }
          handed = true;
          const items = arrange(all.slice(0, MAX_LOCAL), order, seed, pass === 0 ? opts.startId : undefined);
          return { items, done: true, total: items.length };
        },
      };
    },
  };
}

/** Photos a page has already loaded (the viewer's "Play from here", "On this day"), shown from `startId` on in the first pass. */
export function itemsSource(items: readonly SlideItem[], title: string, startId?: string): SlideSource {
  return {
    title,
    open(order, seed, pass): Feed {
      let handed = false;
      return {
        async next() {
          if (handed) return { items: [], done: true };
          handed = true;
          const arranged = arrange(items, order, seed, pass === 0 ? startId : undefined);
          return { items: arranged, done: true, total: arranged.length };
        },
      };
    },
  };
}

/**
 * A search's results. A filter-only search is a scope the server orders and pages; one with words (ranked) or "More like
 * this" is read through its own pages, best match first.
 */
export function searchSource(params: SearchParams, title: string): SlideSource {
  if (!params.q && !params.similar && !params.skip) {
    const filters = { ...params };
    delete filters.q;
    delete filters.similar;
    delete filters.skip;
    return scopeSource({ search: filters }, title);
  }
  return pagedSource(async (cursor) => {
    const r = await api.search(params, { cursor, limit: 100 });
    return { items: r.items, next_cursor: r.next_cursor };
  }, title);
}

/** `items` in `order`; with `startId`, the pass begins at that photo (a shuffle shows it first, then the rest). */
export function arrange(items: readonly SlideItem[], order: SlideOrder, seed: string, startId?: string): SlideItem[] {
  const ordered = order === "oldest" ? oldestFirst(items) : order === "shuffle" ? shuffled(items, seed) : items.slice();
  const at = startId ? ordered.findIndex((i) => i.id === startId) : -1;
  if (at <= 0) return ordered;
  if (order === "shuffle") return [ordered[at]!, ...ordered.slice(0, at), ...ordered.slice(at + 1)];
  // As listed or oldest first: from that photo to the end (the next pass takes the whole set again).
  return ordered.slice(at);
}
