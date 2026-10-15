import { describe, expect, it } from "vitest";
import { trashCaption } from "./trashText";

const DAY = 86_400_000;
const now = Date.parse("2026-03-10T12:00:00Z");
const at = (d: number) => new Date(now + d * DAY).toISOString();

describe("trashCaption", () => {
  it("counts days since deletion and until it is gone", () => {
    expect(trashCaption({ deleted_at: at(-3), purge_at: at(27) }, now)).toBe("Deleted 3 days ago · gone in 27 days");
    expect(trashCaption({ deleted_at: at(-1), purge_at: at(29) }, now)).toBe("Deleted yesterday · gone in 29 days");
    expect(trashCaption({ deleted_at: at(-0.2), purge_at: at(0.8) }, now)).toBe("Deleted today · gone tomorrow");
  });
  it("rounds the time left up and never goes negative", () => {
    expect(trashCaption({ deleted_at: at(-28.5), purge_at: at(1.5) }, now)).toContain("gone in 2 days");
    expect(trashCaption({ deleted_at: at(-31), purge_at: at(-1) }, now)).toBe("Deleted 31 days ago · gone today");
    expect(trashCaption({ deleted_at: at(0.1), purge_at: at(30) }, now)).toContain("Deleted today");
  });
});
