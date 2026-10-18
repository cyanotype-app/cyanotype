import { describe, expect, it } from "vitest";
import type { Invite } from "./api";
import { bytesLabel, expiresIn, gigabytesToBytes, inviteStatusLine, inviteUrl, limitsLine } from "./adminText";
import { parseInviteToken } from "./registration";

const NOW = Date.parse("2026-06-01T12:00:00Z");
const inv = (over: Partial<Invite> = {}): Invite => ({
  id: "i1",
  note: null,
  status: "pending",
  created_at: "2026-06-01T00:00:00Z",
  created_by: "admin@example.com",
  expires_at: "2026-06-07T12:00:00Z",
  revoked_at: null,
  used_at: null,
  used_by: null,
  limits: { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null },
  ...over,
});

describe("bytesLabel", () => {
  it("scales to the largest fitting unit", () => {
    expect(bytesLabel(0)).toBe("0 B");
    expect(bytesLabel(1536)).toBe("1.5 KB");
    expect(bytesLabel(5 * 1024 ** 3)).toBe("5.0 GB");
    expect(bytesLabel(200 * 1024 ** 2)).toBe("200 MB");
    expect(bytesLabel(3 * 1024 ** 4)).toBe("3.0 TB");
  });
});

describe("gigabytesToBytes", () => {
  it("reads a form field", () => {
    expect(gigabytesToBytes("")).toBeNull();
    expect(gigabytesToBytes("  ")).toBeNull();
    expect(gigabytesToBytes("5")).toBe(5 * 1024 ** 3);
    expect(gigabytesToBytes("0,5")).toBe(1024 ** 3 / 2);
    expect(gigabytesToBytes("0")).toBeUndefined();
    expect(gigabytesToBytes("-1")).toBeUndefined();
    expect(gigabytesToBytes("5 GB")).toBeUndefined();
    expect(gigabytesToBytes("1e3")).toBeUndefined();
  });
});

describe("expiresIn", () => {
  it("counts days, hours and minutes", () => {
    expect(expiresIn("2026-06-07T12:00:00Z", NOW)).toBe("in 6 days");
    expect(expiresIn("2026-06-02T12:00:00Z", NOW)).toBe("in 1 day");
    expect(expiresIn("2026-06-01T17:30:00Z", NOW)).toBe("in 5 hours");
    expect(expiresIn("2026-06-01T12:12:00Z", NOW)).toBe("in 12 minutes");
    expect(expiresIn("2026-06-01T12:00:10Z", NOW)).toBe("in 1 minute");
    expect(expiresIn("2026-06-01T11:59:59Z", NOW)).toBe("expired");
  });
});

describe("inviteStatusLine", () => {
  it("says what happened to the invite", () => {
    expect(inviteStatusLine(inv(), NOW)).toBe("Waiting, expires in 6 days");
    expect(inviteStatusLine(inv({ status: "used", used_by: "sam@example.com" }), NOW)).toBe("Used by sam@example.com");
    expect(inviteStatusLine(inv({ status: "used" }), NOW)).toBe("Used");
    expect(inviteStatusLine(inv({ status: "revoked" }), NOW)).toBe("Revoked");
    expect(inviteStatusLine(inv({ status: "expired" }), NOW)).toBe("Expired");
  });
});

describe("limitsLine", () => {
  it("is null without presets and lists the ones set", () => {
    expect(limitsLine(inv().limits)).toBeNull();
    expect(limitsLine({ quota_bytes: 5 * 1024 ** 3, daily_upload_bytes: 1024 ** 3, max_file_bytes: 200 * 1024 ** 2 })).toBe(
      "5.0 GB storage · 1.0 GB a day · files up to 200 MB",
    );
    expect(limitsLine({ quota_bytes: null, daily_upload_bytes: 1024 ** 3, max_file_bytes: null })).toBe("1.0 GB a day");
  });
});

describe("inviteUrl and parseInviteToken", () => {
  it("round-trips a link", () => {
    const token = "ab12".repeat(16);
    const link = inviteUrl("https://photos.example", `/register?invite=${token}`);
    expect(link).toBe(`https://photos.example/register?invite=${token}`);
    expect(parseInviteToken(link)).toBe(token);
  });
  it("accepts a bare token, whitespace and a trailing fragment", () => {
    expect(parseInviteToken("  abc123  ")).toBe("abc123");
    expect(parseInviteToken("https://x.example/register?invite=abc123#top")).toBe("abc123");
    expect(parseInviteToken("https://x.example/register?a=1&invite=abc123&b=2")).toBe("abc123");
    expect(parseInviteToken("")).toBe("");
  });
});
