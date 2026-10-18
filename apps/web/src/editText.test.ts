import { describe, expect, it } from "vitest";
import { OFFSETS, offsetLabel, parsePoint, setDate, shiftSeconds, shiftText } from "./editText";

describe("edit helpers", () => {
  it("offers every half hour from -12:00 to +14:00", () => {
    expect(OFFSETS[0]).toBe("-12:00");
    expect(OFFSETS).toContain("+05:30");
    expect(OFFSETS).toContain("+00:00");
    expect(OFFSETS.at(-1)).toBe("+14:00");
    expect(OFFSETS).toHaveLength(53);
    expect(offsetLabel("+00:00")).toBe("UTC");
    expect(offsetLabel("-07:00")).toBe("UTC-07:00");
  });
  it("turns a shift into signed seconds, ignoring what is not a number", () => {
    expect(shiftSeconds({ later: true, days: 1, hours: 1, minutes: 30 })).toBe(86_400 + 5_400);
    expect(shiftSeconds({ later: false, days: 0, hours: 2, minutes: 0 })).toBe(-7_200);
    expect(shiftSeconds({ later: true, days: NaN, hours: -3, minutes: 1.9 })).toBe(60);
    expect(shiftSeconds({ later: false, days: 0, hours: 0, minutes: 0 })).toBe(-0);
  });
  it("describes a shift", () => {
    expect(shiftText({ later: true, days: 1, hours: 2, minutes: 0 })).toBe("1 day 2 hours later");
    expect(shiftText({ later: false, days: 0, hours: 0, minutes: 1 })).toBe("1 minute earlier");
    expect(shiftText({ later: true, days: 0, hours: 0, minutes: 0 })).toBe("no change");
  });
  it("sends an empty zone as unknown", () => {
    expect(setDate("2023-07-04T13:05", "")).toEqual({ local: "2023-07-04T13:05", offset: null });
    expect(setDate("2023-07-04T13:05", "+02:00")).toEqual({ local: "2023-07-04T13:05", offset: "+02:00" });
  });
  it("reads a pasted point strictly", () => {
    expect(parsePoint("38.7223, -9.1393")).toEqual({ latitude: 38.7223, longitude: -9.1393 });
    expect(parsePoint(" 1 2 ")).toEqual({ latitude: 1, longitude: 2 });
    expect(parsePoint("91, 0")).toBeNull();
    expect(parsePoint("0, 181")).toBeNull();
    expect(parsePoint("Lisbon")).toBeNull();
  });
});
