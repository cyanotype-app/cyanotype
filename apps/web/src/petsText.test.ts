import { describe, expect, it } from "vitest";
import type { PetsSummary } from "./api";
import {
  addedToPetMessage,
  filterablePets,
  mergedPetMessage,
  namedPetLabel,
  offeredKinds,
  petCardLabel,
  petCount,
  petFilterLabel,
  petIdsOf,
  petIdsRuleLabel,
  petKindOf,
  petNameProblem,
  petPhotos,
  petQuestion,
  petRuleLabel,
  petsNamed,
  removedFromPetMessage,
  scanLine,
  scanning,
  scanPercent,
  skippedLines,
} from "./petsText";
import { namedPet } from "./test/fixtures";

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

describe("named pets text", () => {
  const A = "11111111-1111-4111-8111-111111111111";
  const B = "22222222-2222-4222-8222-222222222222";

  it("needs a name, within the server's rules", () => {
    expect(petNameProblem("")).toBe("Give your pet a name.");
    expect(petNameProblem("   ")).toBe("Give your pet a name.");
    expect(petNameProblem("Pet A")).toBeNull();
    expect(petNameProblem("x".repeat(101))).toMatch(/at most 100/);
    expect(petNameProblem("a\u0007b")).toMatch(/control/);
  });

  it("labels a card with the name, the photos and what waits", () => {
    expect(namedPetLabel({ name: "Pet A", photos: 1, pending: 0 })).toBe("Pet A, 1 photo");
    expect(namedPetLabel({ name: "Pet A", photos: 12, pending: 3 })).toBe("Pet A, 12 photos, 3 photos to confirm");
    expect(petQuestion({ name: "Pet A" })).toBe("Is this Pet A?");
  });

  it("reads pet ids out of a URL like people's, and names a filter", () => {
    expect(petIdsOf(`${A},${A},nope,${B.toUpperCase()}`)).toEqual([A, B]);
    expect(petIdsOf(null)).toEqual([]);
    const pets = [{ id: A, name: "Pet A" }, { id: B, name: "Pet B" }];
    expect(petFilterLabel([A], pets)).toBe("Pet A");
    expect(petFilterLabel([A, B], pets)).toBe("Pet A and Pet B");
    expect(petFilterLabel([A, B, "gone"], pets)).toBe("Pet A, Pet B and Unknown pet");
    expect(petIdsRuleLabel([A], pets)).toBe("Pet is Pet A");
    expect(petIdsRuleLabel([A, "gone"], pets)).toBe("Pet: 2 pets together");
    expect(petIdsRuleLabel([A])).toBe("Pet: 1 pet");
  });

  it("offers only pets that are not hidden, and lists each named pet of a photo once", () => {
    expect(filterablePets([namedPet("a", "Pet A"), namedPet("b", "Pet B", 1, { hidden: true })]).map((p) => p.id)).toEqual(["a"]);
    const animal = (id: string, pet: { id: string; name: string } | null) => ({ id, kind: "dog" as const, box: [0, 0, 1, 1] as [number, number, number, number], score: 0.9, pet });
    expect(petsNamed([animal("1", { id: "p", name: "Pet A" }), animal("2", null), animal("3", { id: "p", name: "Pet A" })])).toEqual([{ id: "p", name: "Pet A" }]);
    expect(petsNamed(undefined)).toEqual([]);
  });

  it("words the toasts", () => {
    expect(addedToPetMessage("Pet A", 3, true)).toBe("Made Pet A with 3 photos");
    expect(addedToPetMessage("Pet A", 0, true)).toBe("Made Pet A");
    expect(addedToPetMessage("Pet A", 1, false)).toBe("Added 1 photo to Pet A");
    expect(removedFromPetMessage(2, "Pet A")).toBe("Took 2 photos out of Pet A");
    expect(mergedPetMessage("Pet B")).toBe("Merged into Pet B");
  });

  it("says in plain words why photos were left out", () => {
    const skipped = [
      { asset_id: "1", reason: "no_animal" as const },
      { asset_id: "2", reason: "no_animal" as const },
      { asset_id: "3", reason: "not_scanned" as const },
      { asset_id: "4", reason: "already" as const },
      { asset_id: "5", reason: "in_other_pet" as const },
    ];
    expect(skippedLines(skipped, { name: "Pet A", kind: "cat" })).toEqual([
      "2 photos have no cat that was found",
      "1 photo has not been looked through for pets yet",
      "1 photo is already in Pet A",
      "1 photo belongs to another pet already",
    ]);
    expect(skippedLines([], { name: "Pet A", kind: "dog" })).toEqual([]);
  });
});
