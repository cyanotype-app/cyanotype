import { describe, expect, it } from "vitest";
import { downloadText, expiryFromPreset, expiryText, shareName, shareSummary, shareUrl } from "./shareText";
import { link } from "./test/shareFixtures";

const now = new Date("2026-03-10T12:00:00Z");
const at = (ms: number) => new Date(now.getTime() + ms).toISOString();
const H = 3_600_000;
const D = 24 * H;

describe("expiry presets", () => {
  it("counts from now, and never means no expiry", () => {
    expect(expiryFromPreset("1d", now)).toBe("2026-03-11T12:00:00.000Z");
    expect(expiryFromPreset("1w", now)).toBe("2026-03-17T12:00:00.000Z");
    expect(expiryFromPreset("1m", now)).toBe("2026-04-09T12:00:00.000Z");
    expect(expiryFromPreset("never", now)).toBeNull();
  });
});

describe("expiryText", () => {
  it("says when a link ends or ended", () => {
    expect(expiryText(null, now)).toBe("Never expires");
    expect(expiryText(at(3 * D), now)).toBe("Expires in 3 days");
    expect(expiryText(at(D + 2 * H), now)).toBe("Expires in 1 day");
    expect(expiryText(at(5 * H), now)).toBe("Expires in 5 hours");
    expect(expiryText(at(20 * 60_000), now)).toBe("Expires within the hour");
    expect(expiryText(at(-2 * H), now)).toBe("Expired today");
    expect(expiryText(at(-3 * D - H), now)).toBe("Expired 3 days ago");
  });
});

describe("link text", () => {
  it("builds the public address from the token", () => {
    expect(shareUrl("abc", "https://photos.example")).toBe("https://photos.example/s/abc");
  });
  it("names a link by its title, its album, or what it holds", () => {
    expect(shareName(link(1, { title: "Trip" }))).toBe("Trip");
    expect(shareName(link(1, { kind: "album", album: { id: "a", title: "Summer" } }))).toBe("Summer");
    expect(shareName(link(1, { item_count: 1 }))).toBe("1 photo");
  });
  it("summarises and states the download policy in words", () => {
    expect(shareSummary(link(1, { view_count: 1 }), now)).toBe("Selection · 3 photos · Never expires · 1 view");
    expect(downloadText({ allow_download: false, include_originals: false })).toBe("No downloads");
    expect(downloadText({ allow_download: true, include_originals: false })).toBe("Downloads: JPEG without location");
    expect(downloadText({ allow_download: true, include_originals: true })).toBe("Downloads: originals, with location");
  });
});
