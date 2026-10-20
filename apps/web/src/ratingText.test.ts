import { describe, expect, it } from "vitest";
import { afterTap, clampStars, ratedMessage, ratingFilterLabel, ratingKey, starsLabel } from "./ratingText";

const key = (k: string, mods: Partial<KeyboardEvent> = {}) => ({ key: k, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });

describe("ratingText", () => {
  it("words the rating", () => {
    expect(starsLabel(0)).toBe("Not rated");
    expect(starsLabel(undefined)).toBe("Not rated");
    expect(starsLabel(1)).toBe("1 star");
    expect(starsLabel(4)).toBe("4 stars");
  });
  it("keeps a rating inside 0 to 5", () => {
    expect([clampStars(-2), clampStars(7), clampStars(2.6), clampStars(NaN), clampStars(undefined)]).toEqual([0, 5, 3, 0, 0]);
  });
  it("reads only bare digits 0 to 5 as a rating", () => {
    expect([0, 1, 2, 3, 4, 5].map((n) => ratingKey(key(String(n))))).toEqual([0, 1, 2, 3, 4, 5]);
    expect(ratingKey(key("6"))).toBeNull();
    expect(ratingKey(key("a"))).toBeNull();
    expect(ratingKey(key("12"))).toBeNull();
    expect(ratingKey(key("3", { ctrlKey: true }))).toBeNull();
    expect(ratingKey(key("3", { metaKey: true }))).toBeNull();
    expect(ratingKey(key("3", { altKey: true }))).toBeNull();
    expect(ratingKey(key("3", { shiftKey: true }))).toBeNull();
  });
  it("clears the rating when its own star is tapped again", () => {
    expect(afterTap(3, 3)).toBe(0);
    expect(afterTap(3, 4)).toBe(4);
    expect(afterTap(undefined, 1)).toBe(1);
    expect(afterTap(0, 5)).toBe(5);
  });
  it("words the toast and the filter chip", () => {
    expect(ratedMessage(1, 4)).toBe("1 photo rated 4 stars");
    expect(ratedMessage(1234, 1)).toBe("1,234 photos rated 1 star");
    expect(ratedMessage(2, 0)).toBe("Rating cleared on 2 photos");
    expect(ratingFilterLabel(1)).toBe("1 star & up");
    expect(ratingFilterLabel(4)).toBe("4 stars & up");
    expect(ratingFilterLabel(5)).toBe("5 stars");
  });
});
