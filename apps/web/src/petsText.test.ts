import { describe, expect, it } from "vitest";
import type { PetsSummary } from "./api";
import { offeredKinds, petCardLabel, petCount, petKindOf, petPhotos, petRuleLabel, scanLine, scanning, scanPercent } from "./petsText";

const cover = { asset_id: "a", box: [0.1, 0.1, 0.3, 0.3] as [number, number, number, number], preview_url: "https://s3.test/p" };
const summary = (dogs: number, cats: number, scanned = 10, total = 10): PetsSummary => ({
  any: dogs + cats,
  kinds: [
    { kind: "dog", photos: dogs, cover: dogs ? cover : null },
    { kind: "cat", photos: cats, cover: cats ? cover : null },
  ],
  scanned,
  total,
});

describe("pets text", () => {
  it("reads a kind out of a URL and drops anything else", () => {
    expect(petKindOf("dog")).toBe("dog");
    expect(petKindOf("cat")).toBe("cat");
    expect(petKindOf("any")).toBe("any");
    expect(petKindOf("bird")).toBeUndefined();
    expect(petKindOf("")).toBeUndefined();
    expect(petKindOf(null)).toBeUndefined();
  });

  it("counts photos per kind", () => {
    expect(petCount(summary(3, 2), "any")).toBe(5);
    expect(petCount(summary(3, 2), "dog")).toBe(3);
    expect(petCount(summary(3, 0), "cat")).toBe(0);
    expect(petPhotos(1)).toBe("1 photo");
    expect(petPhotos(12345)).toBe("12,345 photos");
    expect(petCardLabel("dog", 3)).toBe("Dogs, 3 photos");
  });

  it("offers All and only the kinds that have photos", () => {
    expect(offeredKinds(summary(3, 2))).toEqual(["any", "dog", "cat"]);
    expect(offeredKinds(summary(3, 0))).toEqual(["any", "dog"]);
    expect(offeredKinds(summary(0, 0))).toEqual(["any"]);
  });

  it("reports the scan: going, how far, and done", () => {
    expect(scanning({ scanned: 10, total: 100 })).toBe(true);
    expect(scanning({ scanned: 100, total: 100 })).toBe(false);
    expect(scanning({ scanned: 0, total: 0 })).toBe(false);
    expect(scanPercent({ scanned: 25, total: 100 })).toBe(25);
    expect(scanPercent({ scanned: 999, total: 1000 })).toBe(99);
    expect(scanPercent({ scanned: 5, total: 5 })).toBe(100);
    expect(scanPercent({ scanned: 0, total: 0 })).toBeNull();
    expect(scanLine({ scanned: 1200, total: 5000 })).toBe("Looked at 1,200 of 5,000 photos");
  });

  it("names the smart rule", () => {
    expect(petRuleLabel("any")).toBe("Pets: dogs or cats");
    expect(petRuleLabel("dog")).toBe("Pets: dogs");
    expect(petRuleLabel("cat")).toBe("Pets: cats");
  });
});
