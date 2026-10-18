import { describe, expect, it } from "vitest";
import type { BurstMember, SearchItem } from "./api";
import { allPhotos, collapsedStats, indexResults, memberCount, patchPhoto, removePhotos, selectionIds, visiblePhotos } from "./burst";
import { item } from "./test/fixtures";

const member = (n: number): BurstMember => ({ ...item(n), score: 0.5 });
const single = (n: number): SearchItem => ({ ...item(n), score: 0.9, tier: "strong" });
const burst = (n: number, others: number[]): SearchItem => ({
  ...single(n),
  burst: { size: others.length + 1, members: others.map(member) },
});
const ids = (xs: { id: string }[]) => xs.map((x) => x.id);

describe("bursts in the results", () => {
  const results = [single(1), burst(2, [3, 4]), single(5)];

  it("lists every photo for selection and only the expanded ones' members for the grid", () => {
    expect(ids(allPhotos(results))).toEqual(["id-1", "id-2", "id-3", "id-4", "id-5"]);
    expect(ids(visiblePhotos(results, new Set()))).toEqual(["id-1", "id-2", "id-5"]);
    expect(ids(visiblePhotos(results, new Set(["id-2"])))).toEqual(["id-1", "id-2", "id-3", "id-4", "id-5"]);
    expect(memberCount(results[1]!)).toBe(2);
    expect(memberCount(results[0]!)).toBe(0);
  });

  it("selects a collapsed burst as a whole and its photos one by one once it is expanded", () => {
    expect(selectionIds(results[1]!, new Set())).toEqual(["id-2", "id-3", "id-4"]);
    expect(selectionIds(results[1]!, new Set(["id-2"]))).toEqual(["id-2"]);
    expect(selectionIds(results[0]!, new Set())).toEqual(["id-1"]);
    expect(collapsedStats(results, new Set())).toEqual({ photos: 2, bursts: 1 });
    expect(collapsedStats(results, new Set(["id-2"]))).toEqual({ photos: 0, bursts: 0 });
  });

  it("finds a photo and the cover of its burst", () => {
    const idx = indexResults(results);
    expect(idx.get("id-4")?.cover.id).toBe("id-2");
    expect(idx.get("id-5")?.cover.id).toBe("id-5");
  });

  it("changes a member in place", () => {
    const next = patchPhoto(results, "id-4", { favorite: true });
    expect(next[1]!.burst!.members[1]!.favorite).toBe(true);
    expect(next[1]!.favorite).toBeUndefined();
    expect(patchPhoto(results, "id-1", { favorite: true })[0]!.favorite).toBe(true);
    expect(patchPhoto(results, "missing", { favorite: true })).toEqual(results);
  });

  it("removes photos: a member leaves its burst, a cover hands over to the next photo, a burst of one is a photo", () => {
    const noMember = removePhotos(results, new Set(["id-3"]));
    expect(ids(noMember)).toEqual(["id-1", "id-2", "id-5"]);
    expect(noMember[1]!.burst).toMatchObject({ size: 2 });
    expect(ids(noMember[1]!.burst!.members)).toEqual(["id-4"]);

    const noCover = removePhotos(results, new Set(["id-2"]));
    expect(ids(noCover)).toEqual(["id-1", "id-3", "id-5"]);
    expect(noCover[1]!.tier).toBe("strong");
    expect(noCover[1]!.burst).toMatchObject({ size: 2 });
    expect(ids(noCover[1]!.burst!.members)).toEqual(["id-4"]);

    const lone = removePhotos(results, new Set(["id-2", "id-3"]));
    expect(ids(lone)).toEqual(["id-1", "id-4", "id-5"]);
    expect(lone[1]!.burst).toBeUndefined();

    expect(ids(removePhotos(results, new Set(["id-2", "id-3", "id-4"])))).toEqual(["id-1", "id-5"]);
    expect(removePhotos(results, new Set())).toEqual(results);
  });
});
