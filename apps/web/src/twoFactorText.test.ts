import { describe, expect, it } from "vitest";
import {
  cleanCode,
  codeErrorText,
  codeToSend,
  expiryLine,
  groupSecret,
  looksLikeRecoveryCode,
  recoveryCodesFile,
} from "./twoFactorText";

describe("two-factor text", () => {
  it("groups the secret in fours", () => {
    expect(groupSecret("ABCDEFGHIJKLMNOP")).toBe("ABCD EFGH IJKL MNOP");
    expect(groupSecret("ABCDE")).toBe("ABCD E");
  });

  it("accepts six digits with or without a space, nothing else", () => {
    expect(cleanCode("123 456")).toBe("123456");
    expect(cleanCode(" 000000 ")).toBe("000000");
    for (const bad of ["12345", "1234567", "12345a", "", "abcd-efgh"]) expect(cleanCode(bad)).toBeNull();
  });

  it("sends digits as digits and anything else as typed", () => {
    expect(codeToSend("123 456")).toBe("123456");
    expect(codeToSend("  abcd-efgh-2345-ijkl ")).toBe("abcd-efgh-2345-ijkl");
  });

  it("recognises a recovery code in any case and spacing", () => {
    expect(looksLikeRecoveryCode("abcd-efgh-2345-ijkl")).toBe(true);
    expect(looksLikeRecoveryCode("ABCD EFGH 2345 IJKL")).toBe(true);
    expect(looksLikeRecoveryCode("123456")).toBe(false);
  });

  it("makes a download that names the account and lists every code", () => {
    const f = recoveryCodesFile(["aaaa-bbbb-cccc-dddd", "eeee-ffff-gggg-hhhh"], "a@example.com", "https://photos.example", new Date("2026-03-04T10:00:00Z"));
    expect(f).toContain("Account: a@example.com");
    expect(f).toContain("Made: 2026-03-04");
    expect(f).toContain("aaaa-bbbb-cccc-dddd\neeee-ffff-gggg-hhhh");
  });

  it("describes key expiry", () => {
    expect(expiryLine({ expires_at: null, expired: false })).toBe("No expiry");
    expect(expiryLine({ expires_at: "2031-03-03T00:00:00Z", expired: false })).toMatch(/^Expires /);
    expect(expiryLine({ expires_at: "2020-03-03T00:00:00Z", expired: true })).toMatch(/^Expired /);
  });

  it("explains a refused code", () => {
    expect(codeErrorText(401, "invalid_code", "x")).toMatch(/not right/);
    expect(codeErrorText(429, "two_factor_locked", "x")).toMatch(/Wait a few minutes/);
    expect(codeErrorText(401, "challenge_expired", "x")).toMatch(/timed out/);
    expect(codeErrorText(500, undefined, "boom")).toBe("boom");
  });
});
