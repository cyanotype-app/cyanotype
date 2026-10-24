import { describe, expect, it } from "vitest";
import {
  FIT,
  MAX_SCALE,
  clampView,
  doubleTapScale,
  maxScale,
  pastPreview,
  percentLabel,
  pinchView,
  sharperUrl,
  stepScale,
  wheelFactor,
  zoomAt,
  type Rendition,
} from "./zoomMath";

const stage = { w: 400, h: 600 };
// A landscape photo fitted to the stage's width.
const photo = { w: 400, h: 267 };

describe("maxScale", () => {
  it("allows 4 device pixels per picture pixel, between 2x and 8x fit", () => {
    expect(maxScale(1440, 400, 1)).toBeCloseTo(8, 5); // 14.4 -> capped
    expect(maxScale(1440, 400, 3)).toBeCloseTo(4.8, 5); // 3x screen: 4 * 1440 / 1200
    expect(maxScale(1000, 1000, 1)).toBe(4);
    expect(maxScale(300, 1000, 1)).toBe(2); // a small picture still zooms a little
  });
  it("is 2 when the sizes are unknown, and treats a ratio below 1 as 1", () => {
    expect(maxScale(0, 400, 1)).toBe(2);
    expect(maxScale(1440, 0, 1)).toBe(2);
    expect(maxScale(1440, 400, 0)).toBe(maxScale(1440, 400, 1));
  });
});

describe("doubleTapScale", () => {
  it("is 2x, or 1:1 device pixels when that is more", () => {
    expect(doubleTapScale(1440, 400, 3, 8)).toBe(2); // 1:1 would be 1.2
    expect(doubleTapScale(4000, 800, 1, 8)).toBe(5);
  });
  it("never goes past the limit", () => {
    expect(doubleTapScale(4000, 400, 1, 4)).toBe(4);
    expect(doubleTapScale(0, 0, 1, 1.5)).toBe(1.5);
  });
});

describe("clampView", () => {
  it("keeps fit centred", () => {
    expect(clampView({ scale: 1, x: 50, y: -20 }, stage, photo)).toEqual(FIT);
  });
  it("lets the photo move only until its edge meets the stage's", () => {
    // 2x: 800 wide in a 400 stage -> 200 each way; 534 tall in a 600 stage -> centred.
    expect(clampView({ scale: 2, x: 999, y: 999 }, stage, photo)).toEqual({ scale: 2, x: 200, y: 0 });
    expect(clampView({ scale: 2, x: -999, y: 10 }, stage, photo)).toEqual({ scale: 2, x: -200, y: 0 });
    // 4x: 1068 tall -> 234 each way.
    expect(clampView({ scale: 4, x: 0, y: 500 }, stage, photo).y).toBe(234);
  });
  it("limits the scale to between fit and the maximum", () => {
    expect(clampView({ scale: 0.5, x: 0, y: 0 }, stage, photo).scale).toBe(1);
    expect(clampView({ scale: 20, x: 0, y: 0 }, stage, photo).scale).toBe(MAX_SCALE);
    expect(clampView({ scale: 20, x: 0, y: 0 }, stage, photo, 3).scale).toBe(3);
  });
});

describe("zoomAt", () => {
  it("keeps the point under the cursor still", () => {
    const at = { x: 100, y: 50 };
    const v = zoomAt(FIT, 3, at, { w: 400, h: 300 }, { w: 400, h: 300 });
    // The photo point under `at` before: q = at. After: 3 * q + x = at.
    expect(3 * at.x + v.x).toBeCloseTo(at.x, 5);
    expect(3 * at.y + v.y).toBeCloseTo(at.y, 5);
  });
  it("zooming at the centre does not move the photo", () => {
    expect(zoomAt(FIT, 2, { x: 0, y: 0 }, stage, photo)).toEqual({ scale: 2, x: 0, y: 0 });
  });
  it("is held back by the edges: a corner zoom cannot show empty space", () => {
    const v = zoomAt(FIT, 2, { x: 200, y: 0 }, stage, photo); // the right edge
    expect(v.x).toBe(-200);
  });
  it("zooming back out to fit recentres", () => {
    const zoomed = zoomAt(FIT, 4, { x: 150, y: 100 }, stage, photo);
    expect(zoomAt(zoomed, 1, { x: 150, y: 100 }, stage, photo)).toEqual(FIT);
  });
  it("respects the maximum", () => {
    expect(zoomAt(FIT, 50, { x: 0, y: 0 }, stage, photo, 4).scale).toBe(4);
  });
});

describe("pinchView", () => {
  const from: [{ x: number; y: number }, { x: number; y: number }] = [
    { x: -50, y: 0 },
    { x: 50, y: 0 },
  ];
  it("scales by the change in finger distance", () => {
    const to: [{ x: number; y: number }, { x: number; y: number }] = [
      { x: -100, y: 0 },
      { x: 100, y: 0 },
    ];
    expect(pinchView(FIT, from, to, stage, photo).scale).toBeCloseTo(2, 5);
  });
  it("carries the photo with the midpoint, clamped", () => {
    const to: [{ x: number; y: number }, { x: number; y: number }] = [
      { x: 50, y: 0 },
      { x: 150, y: 0 },
    ]; // same distance, moved 100 right: nothing to pan at fit
    expect(pinchView(FIT, from, to, stage, photo)).toEqual(FIT);
    const start = { scale: 2, x: 0, y: 0 };
    expect(pinchView(start, from, to, stage, photo).x).toBe(100);
  });
  it("cannot go below fit or past the maximum", () => {
    const tiny: [{ x: number; y: number }, { x: number; y: number }] = [
      { x: -1, y: 0 },
      { x: 1, y: 0 },
    ];
    expect(pinchView(FIT, from, tiny, stage, photo).scale).toBe(1);
    const huge: [{ x: number; y: number }, { x: number; y: number }] = [
      { x: -5000, y: 0 },
      { x: 5000, y: 0 },
    ];
    expect(pinchView(FIT, from, huge, stage, photo, 4).scale).toBe(4);
  });
  it("ignores two fingers on the same spot", () => {
    const same: [{ x: number; y: number }, { x: number; y: number }] = [
      { x: 0, y: 0 },
      { x: 0, y: 0 },
    ];
    expect(pinchView(FIT, same, [{ x: 0, y: 0 }, { x: 40, y: 0 }], stage, photo).scale).toBe(1);
  });
});

describe("stepScale", () => {
  it("steps by 1.5 and snaps to fit and to the maximum", () => {
    expect(stepScale(1, 1, 8)).toBe(1.5);
    expect(stepScale(1.5, 1, 8)).toBe(2.25);
    expect(stepScale(6, 1, 8)).toBe(8);
    expect(stepScale(8, 1, 8)).toBe(8);
    expect(stepScale(1.5, -1, 8)).toBe(1);
    expect(stepScale(1, -1, 8)).toBe(1);
    expect(stepScale(4, -1, 8)).toBeCloseTo(2.667, 2);
  });
});

describe("wheelFactor", () => {
  it("zooms in on a negative delta and out on a positive one", () => {
    expect(wheelFactor(-10, 0)).toBeGreaterThan(1);
    expect(wheelFactor(10, 0)).toBeLessThan(1);
    expect(wheelFactor(0, 0)).toBe(1);
  });
  it("is gentle for a trackpad and bounded for a wheel notch", () => {
    expect(wheelFactor(-2, 0)).toBeCloseTo(1.02, 2);
    expect(wheelFactor(-100, 0)).toBeCloseTo(Math.exp(0.6), 5);
    expect(wheelFactor(-3, 1)).toBeCloseTo(Math.exp(0.48), 5); // three lines
  });
});

describe("percentLabel", () => {
  it("shows fit as 100 %", () => {
    expect(percentLabel(1)).toBe("100 %");
    expect(percentLabel(2.25)).toBe("225 %");
    expect(percentLabel(1.004)).toBe("100 %");
  });
});

describe("pastPreview", () => {
  it("is never true at fit", () => {
    expect(pastPreview(1, 5000, 1440, 3)).toBe(false);
  });
  it("is true once the drawn photo has more device pixels than the preview", () => {
    expect(pastPreview(2, 390, 1440, 1)).toBe(false); // 780
    expect(pastPreview(4, 390, 1440, 1)).toBe(true); // 1560
    expect(pastPreview(2, 390, 1440, 3)).toBe(true); // 2340
  });
  it("is false while the preview's size is unknown", () => {
    expect(pastPreview(4, 390, 0, 1)).toBe(false);
  });
});

describe("sharperUrl", () => {
  const base: Rendition = {
    contentType: "image/jpeg",
    edited: false,
    rendering: false,
    originalUrl: "https://bucket.example/original?sig=1",
    width: 4000,
    height: 3000,
    previewLong: 1440,
  };
  it("offers the original of an unedited JPEG, PNG or WebP with more pixels than the preview", () => {
    expect(sharperUrl(base)).toBe(base.originalUrl);
    expect(sharperUrl({ ...base, contentType: "image/png" })).toBe(base.originalUrl);
    expect(sharperUrl({ ...base, contentType: "image/webp" })).toBe(base.originalUrl);
    expect(sharperUrl({ ...base, contentType: "IMAGE/JPEG; charset=binary" })).toBe(base.originalUrl);
  });
  it("never offers what a browser cannot show", () => {
    for (const contentType of ["image/heic", "image/heif", "image/x-canon-cr3", "image/x-adobe-dng", "image/tiff", "video/mp4", "image/gif", ""]) {
      expect(sharperUrl({ ...base, contentType }), contentType).toBeNull();
    }
  });
  it("never offers the original of an edited photo, or one being re-rendered", () => {
    expect(sharperUrl({ ...base, edited: true })).toBeNull();
    expect(sharperUrl({ ...base, rendering: true })).toBeNull();
  });
  it("needs a URL, known sizes and a real gain", () => {
    expect(sharperUrl({ ...base, originalUrl: null })).toBeNull();
    expect(sharperUrl({ ...base, width: null })).toBeNull();
    expect(sharperUrl({ ...base, previewLong: 0 })).toBeNull();
    expect(sharperUrl({ ...base, width: 1500, height: 1000 })).toBeNull(); // 1500 is not worth a second download
    expect(sharperUrl({ ...base, width: 1600, height: 1000 })).toBe(base.originalUrl);
  });
  it("takes the long edge, so a portrait file counts", () => {
    expect(sharperUrl({ ...base, width: 3000, height: 4000 })).toBe(base.originalUrl);
  });
  it("skips pictures too large to decode comfortably", () => {
    expect(sharperUrl({ ...base, width: 12000, height: 9000 })).toBeNull();
  });
});
