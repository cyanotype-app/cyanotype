import { describe, expect, it } from "vitest";
import type { SmartRules } from "./api";
import { MAX_RULES, NO_TEXT_QUERY_NOTE, blankRule, blankRules, cleanRules, describeRules, ruleFieldsFor, ruleLabel, rulesProblem, smartCount } from "./smartText";

const rules = (r: SmartRules["rules"], over: Partial<SmartRules> = {}): SmartRules => ({ match: "all", archived: "include", rules: r, ...over });

describe("smart album words", () => {
  it("names every kind of rule", () => {
    expect(ruleLabel({ field: "date", from: "2024-01-01", to: "2024-12-31" })).toBe("2024-01-01 to 2024-12-31");
    expect(ruleLabel({ field: "date", from: "2024-01-01" })).toBe("From 2024-01-01");
    expect(ruleLabel({ field: "date", to: "2024-12-31" })).toBe("Until 2024-12-31");
    expect(ruleLabel({ field: "place", value: "Lisbon" })).toBe("Place: Lisbon");
    expect(ruleLabel({ field: "type", value: "raw" })).toBe("Type: RAW");
    expect(ruleLabel({ field: "favorite", value: true })).toBe("Favorites");
    expect(ruleLabel({ field: "rating", min: 4 })).toBe("4 stars & up");
    expect(ruleLabel({ field: "tags", names: ["a", "b"], mode: "any" })).toBe("Tags: a or b");
    expect(ruleLabel({ field: "has_location", value: false })).toBe("No location");
    expect(ruleLabel({ field: "has_text", value: true })).toBe("Has text in the photo");
    expect(ruleLabel({ field: "caption", text: "cake" })).toBe("Caption has “cake”");
    expect(ruleLabel({ field: "people", ids: ["a", "b"] })).toBe("People: 2 people together");
  });
  it("offers the people rule only while People is on", () => {
    expect(ruleFieldsFor(true).map((f) => f.field)).toContain("people");
    expect(ruleFieldsFor(false).map((f) => f.field)).not.toContain("people");
  });
  it("describes the whole album with its join and the archived switch", () => {
    const r = [{ field: "place" as const, value: "Lisbon" }, { field: "favorite" as const, value: true as const }];
    expect(describeRules(rules(r))).toBe("Place: Lisbon and Favorites");
    expect(describeRules(rules(r, { match: "any", archived: "exclude" }))).toBe("Place: Lisbon or Favorites (without archived)");
  });
  it("counts, with a plus when it is capped", () => {
    expect(smartCount(1, false)).toBe("1 photo");
    expect(smartCount(1234, false)).toBe("1,234 photos");
    expect(smartCount(10000, true)).toBe("10,000+ photos");
  });
});

describe("smart album rules before they are sent", () => {
  it("starts with a usable rule and says what is missing", () => {
    expect(rulesProblem(blankRules())).toBeNull();
    expect(rulesProblem(rules([]))).toMatch(/at least one rule/);
    expect(rulesProblem(rules(Array.from({ length: MAX_RULES + 1 }, () => blankRule("favorite"))))).toMatch(/At most/);
    expect(rulesProblem(rules([blankRule("date")]))).toMatch(/start or an end/);
    expect(rulesProblem(rules([{ field: "date", from: "2024-05-02", to: "2024-05-01" }]))).toMatch(/starts after/);
    expect(rulesProblem(rules([blankRule("place")]))).toMatch(/needs a value/);
    expect(rulesProblem(rules([blankRule("caption")]))).toMatch(/some text/);
    expect(rulesProblem(rules([blankRule("tags")]))).toMatch(/at least one tag/);
    expect(rulesProblem(rules([blankRule("people")]))).toMatch(/at least one person/);
    expect(rulesProblem(rules([{ field: "people", ids: ["a"] }]))).toBeNull();
    expect(rulesProblem(rules([{ field: "people", ids: Array.from({ length: 11 }, (_, i) => `p${i}`) }]))).toMatch(/at most 10/);
    expect(rulesProblem(rules([{ field: "tags", names: ["a,b"], mode: "all" }]))).toMatch(/comma/);
    expect(rulesProblem(rules([blankRule("rating"), blankRule("type"), blankRule("has_text")]))).toBeNull();
  });
  it("leaves empty dates out of what is sent", () => {
    expect(cleanRules(rules([{ field: "date", from: "2024-01-01", to: "" }])).rules).toEqual([{ field: "date", from: "2024-01-01" }]);
    expect(cleanRules(rules([blankRule("favorite")])).rules).toEqual([{ field: "favorite", value: true }]);
  });
  it("says why a text search is not a rule", () => {
    expect(NO_TEXT_QUERY_NOTE).toMatch(/ranking is not stable/);
  });
});
