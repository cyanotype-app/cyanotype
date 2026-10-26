import { describe, expect, it } from "vitest";
import { returnToOf, safeReturnTo } from "./returnTo";

describe("safeReturnTo", () => {
  it("keeps paths on this site, with their query and fragment", () => {
    for (const ok of [
      "/library",
      "/activate",
      "/activate#code=WDJB-MJHT",
      "/cli/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback&state=st-1&code_challenge=abc_-",
      "/albums/3f2c?x=1#top",
    ]) {
      expect(safeReturnTo(ok), ok).toBe(ok);
    }
  });

  it("refuses anything that could leave the site or is not a path", () => {
    for (const bad of [
      "",
      "library",
      "//evil.example",
      "//evil.example/activate",
      "/\\evil.example",
      "\\\\evil.example",
      "https://evil.example/activate",
      "javascript:alert(1)",
      "/..//evil.example",
      "/a/../../b",
      "/a b",
      "/a\nb",
      "/a\u0000b",
      "/" + "a".repeat(3000),
    ]) {
      expect(safeReturnTo(bad), JSON.stringify(bad)).toBeNull();
    }
    expect(safeReturnTo(undefined)).toBeNull();
    expect(safeReturnTo(42)).toBeNull();
  });

  it("reads the router state", () => {
    expect(returnToOf({ from: "/activate#code=WDJB-MJHT" })).toBe("/activate#code=WDJB-MJHT");
    expect(returnToOf({ from: "//evil.example" })).toBeNull();
    expect(returnToOf(null)).toBeNull();
    expect(returnToOf({})).toBeNull();
  });
});
