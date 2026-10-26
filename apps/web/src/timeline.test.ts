import { describe, expect, it } from "vitest";
import type { AssetListItem } from "./api";
import {
  addPage,
  beforeDay,
  buildIndex,
  buildLayout,
  DAY_HEADER_PX,
  dayLabel,
  dayOfIndex,
  dayOfTime,
  dayTop,
  emptyLoaded,
  evict,
  firstMissing,
  fractionOf,
  gridGeometry,
  justifiedRows,
  MONTH_HEADER_PX,
  monthAt,
  monthLabel,
  offsetOf,
  photoRange,
  removeIds,
  requestFor,
  rowAt,
  runAround,
  scrubMarks,
  type Loaded,
} from "./timeline";
import { item } from "./test/fixtures";

// Newest first: Oct 14 (3 photos), Oct 2 (1), Dec 31 2025 (2), Jan 5 2025 (5).
const DAYS = [
  { day: "2026-10-14", count: 3 },
  { day: "2026-10-02", count: 1 },
  { day: "2025-12-31", count: 2 },
  { day: "2025-01-05", count: 5 },
];
const idx = buildIndex(DAYS);
const G = { cols: 4, tile: 100, gap: 2 };

describe("dayTop", () => {
  const layout = buildLayout(
    buildIndex([
      { day: "2024-06-10", count: 3 },
      { day: "2024-06-08", count: 1 },
      { day: "2024-05-01", count: 2 },
    ]),
    gridGeometry(1000),
  );
  const tops = layout.rows.filter((r) => r.kind === "day").map((r) => r.top);

  it("is the header of the day, or of the nearest day with photos before it", () => {
    expect(dayTop(layout.rows, "2024-06-08")).toBe(tops[1]);
    // 9 June has no photos: the page shows the next older day, 8 June.
    expect(dayTop(layout.rows, "2024-06-09")).toBe(tops[1]);
    expect(dayTop(layout.rows, "2024-06-10")).toBe(tops[0]);
  });

  it("clamps to the ends", () => {
    expect(dayTop(layout.rows, "2030-01-01")).toBe(tops[0]); // newer than everything
    expect(dayTop(layout.rows, "2001-01-01")).toBe(tops[2]); // older than everything
    expect(dayTop([], "2024-06-10")).toBeNull();
  });
});

describe("labels and days", () => {
  it("formats month and day headers in UTC", () => {
    expect(monthLabel("2026-10")).toBe("October 2026");
    expect(monthLabel("2026-10-14")).toBe("October 2026");
    expect(dayLabel("2026-10-14")).toBe("Wed, Oct 14");
    expect(dayLabel("2025-01-01")).toBe("Wed, Jan 1");
  });

  it("buckets timestamps by their UTC day, the way the server counts", () => {
    expect(dayOfTime("2026-10-14T23:59:59Z")).toBe("2026-10-14");
    expect(dayOfTime("2026-10-15T00:00:00+00:00")).toBe("2026-10-15");
    expect(dayOfTime("2026-10-14T23:30:00-05:00")).toBe("2026-10-15");
    expect(dayOfTime("nope")).toBe("");
  });

  it("asks for the page that starts at a day with a `before` at the end of that day", () => {
    expect(beforeDay("2026-10-14")).toBe("2026-10-15T00:00:00.000Z");
    expect(beforeDay("2025-12-31")).toBe("2026-01-01T00:00:00.000Z");
    expect(beforeDay("2024-02-29")).toBe("2024-03-01T00:00:00.000Z");
  });
});

describe("index", () => {
  it("numbers photos across days and finds the day of any index", () => {
    expect(idx.total).toBe(11);
    expect(idx.starts).toEqual([0, 3, 4, 6]);
    const day = (i: number) => idx.days[dayOfIndex(idx, i)]?.day;
    expect([0, 2, 3, 4, 5, 6, 10].map(day)).toEqual([
      "2026-10-14",
      "2026-10-14",
      "2026-10-02",
      "2025-12-31",
      "2025-12-31",
      "2025-01-05",
      "2025-01-05",
    ]);
    expect(dayOfIndex(idx, -1)).toBe(-1);
    expect(dayOfIndex(idx, 11)).toBe(-1);
  });

  it("ignores days without photos", () => {
    expect(buildIndex([{ day: "2026-01-01", count: 0 }, { day: "2025-01-01", count: 2 }]).days).toHaveLength(1);
    expect(buildIndex([]).total).toBe(0);
  });
});

describe("layout", () => {
  const layout = buildLayout(idx, G);

  it("puts a month header, then per day a day header and rows of tiles", () => {
    expect(layout.rows.map((r) => (r.kind === "tiles" ? `tiles@${r.start}x${r.count}` : r.kind))).toEqual([
      "month", // October 2026
      "day",
      "tiles@0x3",
      "day",
      "tiles@3x1",
      "month", // December 2025
      "day",
      "tiles@4x2",
      "month", // January 2025
      "day",
      "tiles@6x4",
      "tiles@10x1",
    ]);
  });

  it("gives every row a fixed height, so the total is known before any photo loads", () => {
    const rowH = G.tile + G.gap;
    expect(layout.height).toBe(3 * MONTH_HEADER_PX + 4 * DAY_HEADER_PX + 5 * rowH);
    let top = 0;
    for (const r of layout.rows) {
      expect(r.top).toBe(top);
      top += r.height;
    }
    expect(top).toBe(layout.height);
  });

  it("keeps a month's photo counts and first index for the scrubber", () => {
    expect(layout.months.map((m) => [m.label, m.count, m.first])).toEqual([
      ["October 2026", 4, 0],
      ["December 2025", 2, 4],
      ["January 2025", 5, 6],
    ]);
    expect(layout.months[1]!.top).toBe(layout.rows[layout.months[1]!.row]!.top);
  });

  it("wraps a long day into several rows", () => {
    const big = buildLayout(buildIndex([{ day: "2026-01-01", count: 9 }]), G);
    expect(big.rows.filter((r) => r.kind === "tiles").map((r) => r.kind === "tiles" && r.count)).toEqual([4, 4, 1]);
  });

  it("finds rows, months and photo ranges by offset", () => {
    expect(rowAt(layout.rows, 0)).toBe(0);
    expect(rowAt(layout.rows, MONTH_HEADER_PX + 1)).toBe(1);
    expect(rowAt(layout.rows, layout.height + 500)).toBe(layout.rows.length - 1);
    expect(monthAt(layout, 0)?.label).toBe("October 2026");
    expect(monthAt(layout, layout.months[1]!.top)?.label).toBe("December 2025");
    expect(monthAt(layout, layout.months[1]!.top - 1)?.label).toBe("October 2026");
    expect(monthAt(layout, 1e9)?.label).toBe("January 2025");
    expect(photoRange(layout, 0, 4)).toEqual([0, 3]);
    expect(photoRange(layout, 0, 11)).toEqual([0, 10]);
    expect(photoRange(layout, 0, 1)).toBeNull();
  });

  it("is sized from the container width", () => {
    expect(gridGeometry(320)).toMatchObject({ cols: 3, gap: 2 });
    expect(gridGeometry(390)).toMatchObject({ cols: 3 });
    expect(gridGeometry(414)).toMatchObject({ cols: 3 });
    expect(gridGeometry(520)).toMatchObject({ cols: 4 });
    expect(gridGeometry(600)).toMatchObject({ cols: 4 });
    expect(gridGeometry(768)).toMatchObject({ cols: 4, gap: 4 });
    const wide = gridGeometry(1280);
    expect(wide.cols).toBe(7);
    expect(gridGeometry(1032).cols).toBe(6);
    expect(wide.tile * wide.cols + wide.gap * (wide.cols - 1)).toBeCloseTo(1280);
    const narrow = gridGeometry(320);
    expect(narrow.tile * 3 + narrow.gap * 2).toBeCloseTo(320);
  });

  it("fills the width exactly at every width, with tight gutters", () => {
    for (let w = 280; w <= 1920; w += 7) {
      const g = gridGeometry(w);
      expect(g.tile * g.cols + g.gap * (g.cols - 1)).toBeCloseTo(w);
      expect(g.gap).toBe(w <= 600 ? 2 : 4);
      expect(g.cols).toBeGreaterThanOrEqual(3);
    }
  });

  it("changes the number of columns with the density, within limits", () => {
    expect(gridGeometry(390, "square", 1).cols).toBe(4);
    expect(gridGeometry(390, "square", -1).cols).toBe(2);
    expect(gridGeometry(390, "square", -5).cols).toBe(2);
    expect(gridGeometry(390, "square", 50).cols).toBe(12);
    expect(gridGeometry(390, "justified", 1).rowH).toBeLessThan(gridGeometry(390, "justified").rowH!);
  });
});

const photo = (i: number): AssetListItem => {
  // The photo at global index i, dated on the day the index says.
  const day = idx.days[dayOfIndex(idx, i)]!.day;
  return item(i, { id: `p${i}`, sort_time: `${day}T12:00:00Z` });
};
const loadedOf = (...indices: number[]): Loaded => ({
  items: new Map(indices.map((i) => [i, photo(i)])),
  cursors: new Map(),
});

describe("loading", () => {
  it("finds the first missing photo in a range", () => {
    expect(firstMissing(idx, emptyLoaded, 0, 5)).toBe(0);
    expect(firstMissing(idx, loadedOf(0, 1, 2), 0, 5)).toBe(3);
    expect(firstMissing(idx, loadedOf(0, 1, 2), 0, 2)).toBeNull();
    expect(firstMissing(idx, loadedOf(5), 5, 50)).toBe(6);
    expect(firstMissing(idx, emptyLoaded, 11, 20)).toBeNull(); // past the end
  });

  it("starts a page at the top of the day of the photo it needs, so a jump loads just there", () => {
    expect(requestFor(idx, emptyLoaded, 5)).toEqual({ base: 4, before: "2026-01-01T00:00:00.000Z" });
    expect(requestFor(idx, emptyLoaded, 0)).toEqual({ base: 0, before: "2026-10-15T00:00:00.000Z" });
    expect(requestFor(idx, emptyLoaded, 99)).toBeNull();
  });

  it("continues with the cursor when one is known", () => {
    const l = { ...loadedOf(0, 1), cursors: new Map([[2, "next"]]) };
    expect(requestFor(idx, l, 2)).toEqual({ base: 2, cursor: "next" });
  });

  it("stores a page at its global indices and remembers the cursor for the next one", () => {
    const r = addPage(idx, emptyLoaded, 4, [photo(4), photo(5), photo(6)], "c7");
    expect(r.stale).toBe(false);
    expect([...r.loaded.items.keys()]).toEqual([4, 5, 6]);
    expect(r.loaded.cursors.get(7)).toBe("c7");
    expect(emptyLoaded.items.size).toBe(0); // the input is not mutated
  });

  it("rejects a page whose photos are not on the days the counts say, so the counts get re-read", () => {
    // The first photo of the Dec 31 page is dated Oct 14: something was uploaded since the counts were read.
    const r = addPage(idx, emptyLoaded, 4, [item(1, { sort_time: "2026-10-14T00:00:00Z" })], null);
    expect(r.stale).toBe(true);
    expect(r.loaded).toBe(emptyLoaded);
    expect(addPage(idx, emptyLoaded, 11, [photo(10)], null).stale).toBe(true); // beyond the end
  });

  it("evicts photos far from the viewport but keeps the selection", () => {
    const l = loadedOf(...Array.from({ length: 11 }, (_, i) => i));
    expect(evict(l, 5, 20, 3, new Set())).toBe(l); // under the cap: untouched
    const e = evict(l, 5, 4, 2, new Set(["p10"]));
    expect([...e.items.keys()].sort((a, b) => a - b)).toEqual([3, 4, 5, 6, 7, 10]);
  });

  it("gives the lightbox the unbroken run around a photo", () => {
    const l = loadedOf(2, 3, 4, 5, 8, 9);
    expect(runAround(l, 4, 10)).toMatchObject({ start: 2, items: [{ id: "p2" }, { id: "p3" }, { id: "p4" }, { id: "p5" }] });
    expect(runAround(l, 4, 1)!.items.map((i) => i.id)).toEqual(["p3", "p4", "p5"]);
    expect(runAround(l, 9, 10)).toMatchObject({ start: 8 });
    expect(runAround(l, 6, 10)).toBeNull();
  });
});

describe("removing photos", () => {
  it("closes the gap: later photos move up and the day counts shrink", () => {
    const l = loadedOf(0, 1, 2, 3, 4, 5);
    const r = removeIds(idx, l, new Set(["p1", "p3"]));
    expect(r.removed.map((i) => i.id)).toEqual(["p1", "p3"]);
    expect([...r.loaded.items].map(([i, it]) => [i, it.id])).toEqual([
      [0, "p0"],
      [1, "p2"],
      [2, "p4"],
      [3, "p5"],
    ]);
    expect(r.index.total).toBe(9);
    expect(r.index.days).toEqual([
      { day: "2026-10-14", count: 2 },
      { day: "2025-12-31", count: 2 },
      { day: "2025-01-05", count: 5 },
    ]);
    // Stale cursors are dropped: they pointed at old positions.
    expect(r.loaded.cursors.size).toBe(0);
  });

  it("leaves everything alone when nothing matches", () => {
    const l = loadedOf(0, 1);
    const r = removeIds(idx, l, new Set(["nope"]));
    expect(r.loaded).toBe(l);
    expect(r.index).toBe(idx);
    expect(r.removed).toEqual([]);
  });

  it("keeps every loaded photo on the day the new counts put it on", () => {
    const l = loadedOf(...Array.from({ length: 11 }, (_, i) => i));
    const r = removeIds(idx, l, new Set(["p3"]));
    for (const [i, it] of r.loaded.items) {
      expect(r.index.days[dayOfIndex(r.index, i)]!.day).toBe(dayOfTime(it.sort_time));
    }
    expect(r.index.days.some((d) => d.day === "2026-10-02")).toBe(false); // that day had one photo
    expect(buildLayout(r.index, G).months.map((m) => m.count)).toEqual([3, 2, 5]);
  });
});

describe("scrubber mapping", () => {
  const layout = buildLayout(buildIndex(
    // Ten years, 12 months each, 30 photos per month on one day.
    Array.from({ length: 120 }, (_, k) => {
      const y = 2026 - Math.floor(k / 12);
      const m = 12 - (k % 12);
      return { day: `${y}-${String(m).padStart(2, "0")}-15`, count: 30 };
    }),
  ), { cols: 5, tile: 100, gap: 2 });
  const vh = 800;

  it("maps scroll offsets and track fractions both ways", () => {
    expect(fractionOf(0, layout.height, vh)).toBe(0);
    expect(fractionOf(layout.height - vh, layout.height, vh)).toBe(1);
    expect(fractionOf(layout.height * 5, layout.height, vh)).toBe(1);
    expect(fractionOf(-10, layout.height, vh)).toBe(0);
    expect(fractionOf(100, 500, 800)).toBe(0); // shorter than the viewport: nothing to scroll
    expect(offsetOf(0.5, layout.height, vh)).toBeCloseTo((layout.height - vh) / 2);
    const f = fractionOf(layout.months[60]!.top, layout.height, vh);
    expect(offsetOf(f, layout.height, vh)).toBeCloseTo(layout.months[60]!.top);
  });

  it("labels each year once, in order, and keeps the labels apart", () => {
    const marks = scrubMarks(layout, vh, 600);
    const years = marks.filter((m) => m.year);
    expect(years.map((m) => m.year)).toEqual(["2026", "2025", "2024", "2023", "2022", "2021", "2020", "2019", "2018", "2017"]);
    for (let i = 1; i < years.length; i++) expect(years[i]!.y - years[i - 1]!.y).toBeGreaterThanOrEqual(18);
    expect(marks[0]).toMatchObject({ month: "2026-12", y: 0 });
    // Ticks between the labels: months, never closer than the tick gap.
    for (let i = 1; i < marks.length; i++) expect(marks[i]!.y - marks[i - 1]!.y).toBeGreaterThanOrEqual(6);
    expect(marks.length).toBeGreaterThan(years.length);
    expect(marks.every((m, i) => i === 0 || m.y >= marks[i - 1]!.y)).toBe(true);
  });

  it("drops year labels that do not fit on a short track", () => {
    const marks = scrubMarks(layout, vh, 60);
    expect(marks.filter((m) => m.year).length).toBeLessThanOrEqual(4);
  });
});

describe("justified rows", () => {
  const ratios = (list: number[]) => (k: number) => list[k % list.length]!;

  it("fills every full row exactly and keeps each photo's shape", () => {
    for (const width of [320, 390, 768, 1032, 1600]) {
      const gap = width <= 600 ? 2 : 4;
      const rows = justifiedRows(60, ratios([1.5, 0.75, 1.33, 1, 1.78, 0.67, 2.1]), width, gap, 120);
      expect(rows.reduce((n, r) => n + r.count, 0)).toBe(60);
      for (const r of rows.filter((x) => x.fill)) {
        const sum = r.ratios.reduce((a, b) => a + b, 0);
        expect(sum * r.tileH + gap * (r.count - 1)).toBeCloseTo(width);
        expect(r.tileH).toBeGreaterThan(120 * 0.6);
        expect(r.tileH).toBeLessThan(120 * 1.6);
      }
    }
  });

  it("leaves a short last row at the target height instead of stretching it a long way", () => {
    const rows = justifiedRows(2, ratios([1]), 1000, 4, 100);
    expect(rows).toEqual([{ count: 2, ratios: [1, 1], tileH: 100, fill: false }]);
    const few = justifiedRows(5, ratios([1]), 1000, 4, 100);
    expect(few).toEqual([{ count: 5, ratios: [1, 1, 1, 1, 1], tileH: 100, fill: false }]);
    // Nearly full: it fills.
    const near = justifiedRows(9, ratios([1]), 1000, 4, 100);
    expect(near).toHaveLength(1);
    expect(near[0]).toMatchObject({ count: 9, fill: true });
  });

  it("builds a layout whose rows use the loaded photos' shapes and an estimate for the rest", () => {
    const g = gridGeometry(390, "justified");
    const index = buildIndex([{ day: "2026-10-14", count: 12 }]);
    const wide = (i: number) => item(i, { id: `p${i}`, width: 1000, height: 4000 });
    const estimated = buildLayout(index, g);
    const loaded = buildLayout(index, g, { items: new Map(Array.from({ length: 12 }, (_, i) => [i, wide(i)])), cursors: new Map() });
    const tiles = (l: typeof estimated) => l.rows.filter((r) => r.kind === "tiles");
    expect(tiles(estimated).reduce((n, r) => n + (r.kind === "tiles" ? r.count : 0), 0)).toBe(12);
    // Tall photos (cropped to 1:2): more per row than the 3:2 estimate.
    expect(tiles(loaded)[0]).toMatchObject({ kind: "tiles" });
    expect((tiles(loaded)[0] as { count: number }).count).toBeGreaterThan((tiles(estimated)[0] as { count: number }).count);
    expect(loaded.height).toBeGreaterThan(0);
    expect(loaded.rows.filter((r) => r.kind === "tiles").every((r) => r.kind === "tiles" && r.height > (r.tileH ?? 0))).toBe(true);
  });
});
