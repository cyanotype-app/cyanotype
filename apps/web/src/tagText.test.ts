import { describe, expect, it } from "vitest";
import type { Tag } from "./api";
import { nameProblem, suggest, tagFilterLabel, tagListOf, taggedMessage, tagPhotos, tagSearchHref, tidy, withName } from "./tagText";

const tag = (name: string, count: number): Tag => ({ id: name, name, count });

describe("tag names", () => {
  it("folds white space and checks what the server checks", () => {
    expect(tidy("  Summer   2024 ")).toBe("Summer 2024");
    expect(nameProblem("trip/2024")).toBeNull();
    expect(nameProblem("Café ☕")).toBeNull();
    expect(nameProblem("   ")).toMatch(/needs a name/);
    expect(nameProblem("a,b")).toMatch(/comma/);
    expect(nameProblem("x\u0007y")).toMatch(/control/);
    expect(nameProblem("x".repeat(100))).toBeNull();
    expect(nameProblem("x".repeat(101))).toMatch(/at most 100/);
    expect(nameProblem("é".repeat(100))).toBeNull();
  });
  it("reads a list from an address: distinct ignoring case, bad names dropped, at most 20", () => {
    expect(tagListOf("Beach, sunset ,beach,,")).toEqual(["Beach", "sunset"]);
    expect(tagListOf(null)).toEqual([]);
    expect(tagListOf(Array.from({ length: 30 }, (_, i) => `t${i}`).join(","))).toHaveLength(20);
  });
  it("adds a name once, in any case", () => {
    expect(withName(["Beach"], "beach")).toEqual(["Beach"]);
    expect(withName(["Beach"], " Sun  set ")).toEqual(["Beach", "Sun set"]);
    expect(withName(["a"], "x,y")).toEqual(["a"]);
  });
});

describe("autocomplete and words", () => {
  const tags = [tag("Beach", 5), tag("Barbecue", 9), tag("Sunset beach", 20), tag("Trip", 1)];
  it("offers names containing the text, prefix matches first, then the most used, never one already chosen", () => {
    expect(suggest("be", tags, []).map((t) => t.name)).toEqual(["Beach", "Sunset beach", "Barbecue"]);
    expect(suggest("b", tags, []).map((t) => t.name)).toEqual(["Barbecue", "Beach", "Sunset beach"]);
    expect(suggest("", tags, ["beach"]).map((t) => t.name)).toEqual(["Sunset beach", "Barbecue", "Trip"]);
    expect(suggest("", tags, [], 2)).toHaveLength(2);
  });
  it("words the chip, counts and toast", () => {
    expect(tagFilterLabel("beach")).toBe("Tag: beach");
    expect(tagFilterLabel("beach,sunset")).toBe("Tags: beach and sunset");
    expect(tagFilterLabel("beach,sunset", "any")).toBe("Tags: beach or sunset");
    expect(tagPhotos(1)).toBe("1 photo");
    expect(tagPhotos(1200)).toBe("1,200 photos");
    expect(taggedMessage(3, ["a", "b"], [])).toBe("Added a and b on 3 photos");
    expect(taggedMessage(1, ["a"], ["x", "y", "z"])).toBe("Added a, removed 3 tags on 1 photo");
    expect(tagSearchHref("trip/2024 & more")).toBe("/search?tags=trip%2F2024+%26+more");
  });
});
