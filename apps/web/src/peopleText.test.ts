import { describe, expect, it } from "vitest";
import type { Person } from "./api";
import {
  MAX_FILTER_PEOPLE,
  cropStyle,
  filterablePeople,
  matchingPeople,
  mergedMessage,
  nameProblem,
  openPersonLabel,
  peopleFilterLabel,
  peopleIdsOf,
  peopleRuleLabel,
  peopleWord,
  personLabel,
  photosWord,
  removedMessage,
  splitPeople,
  suggestionQuestion,
  tidyName,
} from "./peopleText";

const person = (id: string, name: string | null, photos = 3, hidden = false): Person => ({ id, name, hidden, faces: photos, photos, cover: null });

describe("names", () => {
  it("checks what the server checks; blank is allowed because it clears the name", () => {
    expect(tidyName("  Person A ")).toBe("Person A");
    expect(nameProblem("")).toBeNull();
    expect(nameProblem("Alpha-Bravo O'Charlie")).toBeNull();
    expect(nameProblem("é".repeat(100))).toBeNull();
    expect(nameProblem("x".repeat(101))).toMatch(/at most 100/);
    expect(nameProblem("a\u0007b")).toMatch(/control/);
  });
  it("calls an unnamed person unnamed", () => {
    expect(personLabel({ name: "Person A" })).toBe("Person A");
    expect(personLabel({ name: null })).toBe("Unnamed person");
    expect(personLabel({ name: "   " })).toBe("Unnamed person");
  });
});

describe("words", () => {
  it("counts", () => {
    expect(photosWord(1)).toBe("1 photo");
    expect(photosWord(12345)).toBe("12,345 photos");
    expect(peopleWord(1)).toBe("1 person");
    expect(peopleWord(0)).toBe("0 people");
    expect(openPersonLabel(person("a", null, 2))).toBe("Unnamed person, 2 photos");
  });
  it("asks and reports", () => {
    expect(suggestionQuestion({ name: "Person A" })).toBe("Is this Person A?");
    expect(suggestionQuestion({ name: null })).toBe("Is this the same person?");
    expect(removedMessage(1, { name: null })).toBe("Took 1 photo out of Unnamed person");
    expect(mergedMessage({ name: "Person B" }, 5)).toBe("Merged into Person B (5 faces)");
    expect(mergedMessage({ name: "Person B" }, 1)).toBe("Merged into Person B (1 face)");
  });
});

describe("the people shown and hidden", () => {
  it("splits in order", () => {
    const l = [person("a", "A"), person("b", null, 1, true), person("c", "C"), person("d", "D", 1, true)];
    const { shown, hidden } = splitPeople(l);
    expect(shown.map((p) => p.id)).toEqual(["a", "c"]);
    expect(hidden.map((p) => p.id)).toEqual(["b", "d"]);
  });
});

describe("cropping a face out of a preview", () => {
  const parse = (s: string) => Number.parseFloat(s);
  it("centres a face with room around it", () => {
    // A 1000 x 500 picture, a 100 x 100 px face at (450, 200): a 170 px square around its centre (500, 250).
    const c = cropStyle([0.45, 0.4, 0.1, 0.2], 1000, 500);
    expect(parse(c.width)).toBeCloseTo((1000 / 170) * 100, 1);
    expect(parse(c.left)).toBeCloseTo(-((500 - 85) / 170) * 100, 1);
    expect(parse(c.top)).toBeCloseTo(-((250 - 85) / 170) * 100, 1);
  });
  it("stays inside the picture when the face is at an edge", () => {
    const c = cropStyle([0, 0, 0.1, 0.1], 1000, 1000, 2);
    expect(parse(c.left)).toBeCloseTo(0);
    expect(parse(c.top)).toBeCloseTo(0);
    const d = cropStyle([0.9, 0.9, 0.1, 0.1], 1000, 1000, 2);
    // 200 px square at the bottom right corner: the picture starts 800 px in, i.e. 4 box widths.
    expect(parse(d.left)).toBeCloseTo(-400);
    expect(parse(d.top)).toBeCloseTo(-400);
  });
  it("centres the picture when the face is bigger than it can be framed", () => {
    // The face fills a 100 x 100 picture; 1.7 times its side does not fit: the picture sits in the middle of the box.
    const c = cropStyle([0, 0, 1, 1], 100, 100);
    expect(parse(c.width)).toBeCloseTo(100 / 1.7, 1);
    expect(parse(c.left)).toBeGreaterThan(0);
  });
  it("never divides by zero", () => {
    const c = cropStyle([0.5, 0.5, 0, 0], 0, 0);
    expect(Number.isFinite(parse(c.width)) && Number.isFinite(parse(c.left))).toBe(true);
  });
});

const ID = (n: number) => `6f1c2b0e-0000-4000-8000-${String(n).padStart(12, "0")}`;

describe("the people filter", () => {
  it("reads ids from the URL: valid ones only, once each, lower case, at most ten", () => {
    expect(peopleIdsOf(`${ID(1)},${ID(2)}`)).toEqual([ID(1), ID(2)]);
    expect(peopleIdsOf(` ${ID(1).toUpperCase()} ,nope,${ID(1)},,${ID(2)}`)).toEqual([ID(1), ID(2)]);
    expect(peopleIdsOf("")).toEqual([]);
    expect(peopleIdsOf(null)).toEqual([]);
    expect(peopleIdsOf(Array.from({ length: 15 }, (_, i) => ID(i + 1)).join(","))).toHaveLength(MAX_FILTER_PEOPLE);
  });

  it("offers named people who are not hidden", () => {
    const list = [person("a", "Person A"), person("b", null), person("c", "  "), person("d", "Person D", 3, true), person("e", "Person E")];
    expect(filterablePeople(list).map((p) => p.id)).toEqual(["a", "e"]);
  });

  it("narrows the picker by name, keeping the chosen people first", () => {
    const list = [person("a", "Person A"), person("b", "Person B"), person("c", "Someone")];
    expect(matchingPeople(list, "", []).map((p) => p.id)).toEqual(["a", "b", "c"]);
    expect(matchingPeople(list, "PERSON", []).map((p) => p.id)).toEqual(["a", "b"]);
    expect(matchingPeople(list, "some", ["b"]).map((p) => p.id)).toEqual(["b", "c"]);
    expect(matchingPeople(list, "zzz", [])).toEqual([]);
  });

  it("labels the chip with the names, and says so for a person who is gone", () => {
    const list = [person("a", "Person A"), person("b", "Person B"), person("c", "Person C")];
    expect(peopleFilterLabel(["a"], list)).toBe("Person A");
    expect(peopleFilterLabel(["a", "b"], list)).toBe("Person A and Person B");
    expect(peopleFilterLabel(["a", "b", "c"], list)).toBe("Person A, Person B and Person C");
    expect(peopleFilterLabel(["a", "x"], list)).toBe("Person A and Unknown person");
    expect(peopleRuleLabel(["a"])).toBe("People: 1 person");
    expect(peopleRuleLabel(["a", "b"])).toBe("People: 2 people together");
  });
});
