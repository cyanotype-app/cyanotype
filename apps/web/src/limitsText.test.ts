import { describe, expect, it } from "vitest";
import type { MyUsage } from "./api";
import { blockText, gigabytesField, levelOf, limitsLine, limitsWords, meters, percentOf, resumeText, warningFor } from "./limitsText";

const GB = 1024 ** 3;
const usage = (over: Partial<MyUsage> = {}): MyUsage => ({
  originals_bytes: 0,
  trash_bytes: 0,
  stored_bytes: 0,
  derived_bytes: 0,
  derived_complete: true,
  photo_count: 0,
  raw_count: 0,
  file_count: 0,
  trash_count: 0,
  uploads: { count_24h: 0, bytes_24h: 0, count_30d: 0, bytes_30d: 0 },
  ...over,
});

describe("levelOf and percentOf", () => {
  it("warns at 80 %, is critical at 95 % and full at the limit", () => {
    expect(levelOf(0, 100)).toBe("ok");
    expect(levelOf(79, 100)).toBe("ok");
    expect(levelOf(80, 100)).toBe("warn");
    expect(levelOf(94, 100)).toBe("warn");
    expect(levelOf(95, 100)).toBe("critical");
    expect(levelOf(99, 100)).toBe("critical");
    expect(levelOf(100, 100)).toBe("full");
    expect(levelOf(150, 100)).toBe("full");
    expect(levelOf(5, 0)).toBe("ok");
  });
  it("rounds down and never passes 100", () => {
    expect(percentOf(799, 1000)).toBe(79);
    expect(percentOf(800, 1000)).toBe(80);
    expect(percentOf(5000, 1000)).toBe(100);
    expect(percentOf(1, 0)).toBe(0);
  });
});

describe("meters and warnings", () => {
  it("has a meter only for a limit that is set", () => {
    const m = meters(usage({ stored_bytes: 4 * GB, limits: { quota_bytes: 5 * GB, daily_upload_bytes: null, max_file_bytes: null } }));
    expect(m.quota).toEqual({ used: 4 * GB, limit: 5 * GB, percent: 80, level: "warn" });
    expect(m.daily).toBeNull();
    expect(m.maxFile).toBeNull();
    expect(meters(usage()).quota).toBeNull();
  });
  it("counts the daily cap from the server's window, else from the 24 hour uploads", () => {
    const limits = { quota_bytes: null, daily_upload_bytes: GB, max_file_bytes: null };
    expect(meters(usage({ limits, daily: { used_bytes: GB / 2, oldest_rolls_out_at: null } })).daily?.percent).toBe(50);
    expect(meters(usage({ limits, uploads: { count_24h: 1, bytes_24h: GB / 4, count_30d: 1, bytes_30d: 0 } })).daily?.percent).toBe(25);
  });
  it("words the 80 %, 95 % and full warnings, and nothing below", () => {
    const m = (used: number) => meters(usage({ stored_bytes: used, limits: { quota_bytes: 100, daily_upload_bytes: null, max_file_bytes: null } })).quota;
    expect(warningFor(m(50), "storage")).toBeNull();
    expect(warningFor(m(82), "storage")).toBe("You have used 82% of your storage.");
    expect(warningFor(m(96), "storage")).toMatch(/almost full: 96% used/);
    expect(warningFor(m(100), "storage")).toMatch(/storage is full.*trash counts/);
    expect(warningFor(m(96), "daily upload limit")).toMatch(/almost at your daily upload limit: 96%/);
    expect(warningFor(m(100), "daily upload limit")).toMatch(/reached your daily upload limit/);
    expect(warningFor(null, "storage")).toBeNull();
  });
});

describe("resumeText and blockText", () => {
  const now = new Date(2026, 5, 10, 12, 0, 0);
  it("says today, tomorrow or the date", () => {
    expect(resumeText(new Date(2026, 5, 10, 15, 4).toISOString(), now)).toMatch(/3:04\s?PM today/);
    expect(resumeText(new Date(2026, 5, 11, 1, 30).toISOString(), now)).toMatch(/1:30\s?AM tomorrow/);
    expect(resumeText(new Date(2026, 5, 14, 9, 0).toISOString(), now)).toMatch(/Jun 14, 9:00\s?AM/);
  });
  it("tells a daily cap from a quota and gives the resume time", () => {
    const at = new Date(2026, 5, 11, 1, 30).toISOString();
    expect(blockText({ code: "daily_cap_exceeded", message: "m", retryAt: at }, now)).toMatch(/Uploads can resume at 1:30\s?AM tomorrow\./);
    expect(blockText({ code: "daily_cap_exceeded", message: "m", retryAt: null }, now)).toMatch(/Try again later/);
    expect(blockText({ code: "quota_exceeded", message: "m", retryAt: null }, now)).toMatch(/Not enough storage.*empty the trash/);
  });
});

describe("limitsLine, limitsWords and gigabytesField", () => {
  it("lists the limits that exist", () => {
    expect(limitsLine(usage())).toBeNull();
    const u = usage({
      stored_bytes: 4 * GB,
      daily: { used_bytes: GB / 2, oldest_rolls_out_at: null },
      limits: { quota_bytes: 5 * GB, daily_upload_bytes: GB, max_file_bytes: 200 * 1024 ** 2 },
    });
    expect(limitsLine(u)).toBe("Storage 4.0 GB of 5.0 GB · last 24 h 512 MB of 1.0 GB · files up to 200 MB");
    expect(limitsWords({})).toBe("No limits");
    expect(limitsWords({ quota_bytes: 5 * GB, max_file_bytes: null })).toBe("5.0 GB storage");
  });
  it("round-trips a limit through the form field", () => {
    expect(gigabytesField(null)).toBe("");
    expect(gigabytesField(5 * GB)).toBe("5");
    expect(gigabytesField(GB / 2)).toBe("0.5");
    expect(gigabytesField(Math.round(1.234 * GB))).toBe("1.234");
  });
});
