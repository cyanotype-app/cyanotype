import { describe, expect, it } from "vitest";
import type { Person } from "../api";
import { albumCount, dateRange, homePeople, isFreshTrip, tripLine, tripName } from "./text";

describe("dateRange", () => {
  it("writes the shortest range that is still unambiguous", () => {
    expect(dateRange("2026-10-12", "2026-10-12")).toBe("Oct 12, 2026");
    expect(dateRange("2026-10-12", "2026-10-19")).toBe("Oct 12 – 19, 2026");
    expect(dateRange("2026-09-28", "2026-10-03")).toBe("Sep 28 – Oct 3, 2026");
    expect(dateRange("2025-12-30", "2026-01-02")).toBe("Dec 30, 2025 – Jan 2, 2026");
  });
});

describe("trip words", () => {
  const t = { title: "New York", date_label: "October 2026", photos: 1234 };
  it("names a trip for the card and for assistive technology", () => {
    expect(tripLine(t)).toBe("October 2026 · 1,234 photos");
    expect(tripName({ ...t, photos: 1 })).toBe("New York, October 2026, 1 photo");
  });
  it("calls a trip fresh while it goes on and for six weeks after", () => {
    expect(isFreshTrip({ ongoing: true, to: "2026-10-26" }, "2026-10-26")).toBe(true);
    expect(isFreshTrip({ ongoing: false, to: "2026-09-14" }, "2026-10-26")).toBe(true);
    expect(isFreshTrip({ ongoing: false, to: "2026-09-13" }, "2026-10-26")).toBe(false);
  });
});

describe("albums and people", () => {
  it("marks a capped smart album's count", () => {
    expect(albumCount({ photo_count: 10000, photo_count_capped: true })).toBe("10,000+ photos");
    expect(albumCount({ photo_count: 1 })).toBe("1 photo");
  });
  const person = (id: string, name: string | null, photos: number, over: Partial<Person> = {}): Person => ({
    id,
    name,
    hidden: false,
    faces: photos,
    photos,
    cover: { face_id: "f", asset_id: "a", box: [0, 0, 1, 1], preview_url: "https://s3.test/p" },
    ...over,
  });
  it("lists named people first, then the biggest, without hidden or faceless ones", () => {
    const list = homePeople([
      person("a", null, 90),
      person("b", "Bravo", 3),
      person("c", "Alpha", 10),
      person("d", "Gone", 50, { hidden: true }),
      person("e", "Blank", 50, { cover: null }),
    ]);
    expect(list.map((p) => p.id)).toEqual(["c", "b", "a"]);
    expect(homePeople([person("a", "A", 1), person("b", "B", 2)], 1).map((p) => p.id)).toEqual(["b"]);
  });
});
