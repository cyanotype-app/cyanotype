import { describe, expect, it } from "vitest";
import { dateRange, photoCount } from "./albumText";

describe("album text", () => {
  it("pluralises photo counts", () => {
    expect([photoCount(0), photoCount(1), photoCount(2)]).toEqual(["0 photos", "1 photo", "2 photos"]);
  });

  it("shows one month, a span, or nothing", () => {
    const one = dateRange("2024-06-10T12:00:00Z", "2024-06-20T12:00:00Z");
    expect(one).not.toContain("–");
    const span = dateRange("2024-06-10T12:00:00Z", "2025-03-20T12:00:00Z");
    expect(span).toContain("–");
    expect(span).toContain("2025");
    expect(dateRange("2024-06-10T12:00:00Z", null)).toBe(one);
    expect(dateRange(null, null)).toBe("");
  });
});
