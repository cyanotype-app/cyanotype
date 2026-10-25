import { afterEach, describe, expect, it, vi } from "vitest";
import { FRAME_KEY, frameParams, frameScope, loadFrameConfig, parseFrameParams, saveFrameConfig } from "./frameConfig";

const ALBUM = "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d";
const q = (s: string) => new URLSearchParams(s);

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("frame address", () => {
  it("reads what the address asks for", () => {
    expect(parseFrameParams(q(`album=${ALBUM}&interval=60&clock=1&caption=1`))).toEqual({
      source: { kind: "album", id: ALBUM },
      interval: 60,
      clock: true,
      caption: true,
    });
    expect(parseFrameParams(q("source=favorites"))).toEqual({ source: { kind: "favorites" }, interval: 30, clock: false, caption: false });
    expect(parseFrameParams(q("source=library&clock=true"))?.clock).toBe(true);
  });

  it("asks the setup screen when it names nothing usable", () => {
    expect(parseFrameParams(q(""))).toBeNull();
    expect(parseFrameParams(q("interval=60"))).toBeNull();
    expect(parseFrameParams(q("source=everything"))).toBeNull();
    expect(parseFrameParams(q("album=not-an-id"))).toBeNull();
  });

  it("keeps the interval within bounds", () => {
    expect(parseFrameParams(q("source=library&interval=1"))?.interval).toBe(5);
    expect(parseFrameParams(q("source=library&interval=999999"))?.interval).toBe(3600);
    expect(parseFrameParams(q("source=library&interval=abc"))?.interval).toBe(30);
  });

  it("round-trips, and names the scope the server pages", () => {
    const c = { source: { kind: "album" as const, id: ALBUM }, interval: 300, clock: true, caption: false };
    expect(parseFrameParams(frameParams(c))).toEqual(c);
    expect(frameScope(c.source)).toEqual({ album: ALBUM });
    expect(frameScope({ kind: "library" })).toBe("library");
    expect(frameScope({ kind: "favorites" })).toBe("favorites");
  });

  it("is remembered in this browser, and storage that throws is no problem", () => {
    const c = { source: { kind: "library" as const }, interval: 10, clock: false, caption: true };
    saveFrameConfig(c);
    expect(localStorage.getItem(FRAME_KEY)).toContain("source=library");
    expect(loadFrameConfig()).toEqual(c);
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(loadFrameConfig()).toBeNull();
    expect(() => saveFrameConfig(c)).not.toThrow();
  });
});
