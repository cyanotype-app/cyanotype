import { describe, expect, it } from "vitest";
import {
  dragCrop,
  fitRatio,
  fromServer,
  IDENTITY,
  isIdentity,
  outputSize,
  RATIOS,
  ratioOf,
  recipeText,
  sameRecipe,
  straightenZoom,
  toRequest,
  turn,
  type Crop,
  type Recipe,
} from "./recipeText";

const crop = (x: number, y: number, w: number, h: number): Crop => ({ x, y, w, h });
const opts = { fw: 4000, fh: 3000, ratio: null };
const r6 = (c: Crop): Crop => ({ x: Math.round(c.x * 1e6) / 1e6, y: Math.round(c.y * 1e6) / 1e6, w: Math.round(c.w * 1e6) / 1e6, h: Math.round(c.h * 1e6) / 1e6 });

describe("the recipe", () => {
  it("is nothing when it changes nothing, and the request says revert", () => {
    expect(isIdentity(IDENTITY)).toBe(true);
    expect(isIdentity({ rotate: 360, straighten: 0.02, crop: crop(0, 0, 1, 1) })).toBe(true);
    expect(toRequest(IDENTITY)).toEqual({ revert: true });
    expect(isIdentity({ rotate: 90, straighten: 0, crop: null })).toBe(false);
  });

  it("is sent in its canonical form", () => {
    expect(toRequest({ rotate: -90, straighten: 1.2345, crop: crop(0.123456, 0, 0.5, 0.5) })).toEqual({
      rotate: 270,
      straighten: 1.2,
      crop: { x: 0.1235, y: 0, w: 0.5, h: 0.5 },
    });
    // A crop of the whole frame is no crop.
    expect(toRequest({ rotate: 90, straighten: 0, crop: crop(0, 0, 1, 1) })).toEqual({ rotate: 90, straighten: 0 });
  });

  it("reads what the server sends and compares recipes by what they do", () => {
    expect(fromServer(null)).toEqual(IDENTITY);
    expect(fromServer({ rotate: 90 })).toEqual({ rotate: 90, straighten: 0, crop: null });
    const a: Recipe = { rotate: 90, straighten: 1, crop: crop(0.1, 0.1, 0.5, 0.5) };
    expect(sameRecipe(a, { ...a, crop: crop(0.1001, 0.1, 0.5, 0.5) })).toBe(true);
    expect(sameRecipe(a, { ...a, crop: null })).toBe(false);
    expect(sameRecipe({ ...a, crop: crop(0, 0, 1, 1) }, { ...a, crop: null })).toBe(true);
    expect(sameRecipe(a, { ...a, rotate: 180 })).toBe(false);
  });

  it("knows the size of the result, as the server does", () => {
    expect(outputSize({ rotate: 90, straighten: 0, crop: crop(0, 0, 0.5, 1) }, 4000, 3000)).toEqual([1500, 4000]);
    expect(outputSize(IDENTITY, 4000, 3000)).toEqual([4000, 3000]);
    expect(outputSize({ rotate: 0, straighten: 5, crop: null }, 4000, 3000)).toEqual([4000, 3000]);
  });

  it("magnifies a straightened picture until no corner shows", () => {
    expect(straightenZoom(0, 400, 300)).toBe(1);
    expect(straightenZoom(45, 100, 100)).toBeCloseTo(Math.SQRT2, 9);
    expect(straightenZoom(-5, 400, 300)).toBe(straightenZoom(5, 400, 300));
    expect(straightenZoom(5, 400, 100)).toBeGreaterThan(straightenZoom(5, 300, 300));
  });

  it("turns the crop with the picture, and four turns are none", () => {
    const r: Recipe = { rotate: 0, straighten: 2, crop: crop(0, 0, 0.25, 0.5) };
    const t = turn(r, 1);
    expect(t.rotate).toBe(90);
    expect(t.crop).toEqual({ x: 0.5, y: 0, w: 0.5, h: 0.25 });
    expect(t.straighten).toBe(2);
    expect(turn(r, 4)).toEqual(r);
    expect(turn(turn(r, 1), -1)).toEqual(r);
    expect(turn(r, -1).rotate).toBe(270);
    // The same arithmetic as the server's test of `turned`.
    expect(turn(turn(turn(t, 1), 1), 1).crop).toEqual(r.crop);
  });

  it("describes itself", () => {
    expect(recipeText(null)).toBe("Not edited");
    expect(recipeText({ rotate: 90, straighten: 0, crop: null })).toBe("Rotated 90° right");
    expect(recipeText({ rotate: 270, straighten: -1.3, crop: crop(0, 0, 0.5, 0.5) }, 4000, 3000)).toBe(
      "Rotated 90° left, straightened 1.3° left, cropped to 1500 × 2000",
    );
    expect(recipeText({ rotate: 180, straighten: 0, crop: null })).toBe("Rotated 180°");
  });
});

describe("dragging the crop", () => {
  const start = crop(0.2, 0.2, 0.5, 0.5);

  it("moves inside the frame and no further", () => {
    expect(r6(dragCrop(start, "move", 0.1, -0.1, opts))).toEqual(crop(0.3, 0.1, 0.5, 0.5));
    expect(r6(dragCrop(start, "move", 5, 5, opts))).toEqual(crop(0.5, 0.5, 0.5, 0.5));
    expect(r6(dragCrop(start, "move", -5, -5, opts))).toEqual(crop(0, 0, 0.5, 0.5));
  });

  it("resizes from a corner or an edge, freely", () => {
    expect(r6(dragCrop(start, "se", 0.1, 0.05, opts))).toEqual(crop(0.2, 0.2, 0.6, 0.55));
    const nw = dragCrop(start, "nw", 0.1, 0.1, opts);
    expect([nw.x, nw.y, nw.w, nw.h].map((v) => Math.round(v * 1e6) / 1e6)).toEqual([0.3, 0.3, 0.4, 0.4]);
    expect(r6(dragCrop(start, "e", 0.1, 0.9, opts))).toEqual(crop(0.2, 0.2, 0.6, 0.5));
    expect(r6(dragCrop(start, "n", 0.9, -0.1, opts))).toEqual(crop(0.2, 0.1, 0.5, 0.6));
  });

  it("never leaves the frame or gets smaller than the minimum", () => {
    const big = dragCrop(start, "se", 5, 5, opts);
    expect([big.x, big.y, big.x + big.w, big.y + big.h]).toEqual([0.2, 0.2, 1, 1]);
    const small = dragCrop(start, "se", -5, -5, opts);
    expect(small.w).toBeCloseTo(0.05, 9);
    expect(small.h).toBeCloseTo(0.05, 9);
    const w = dragCrop(start, "w", -5, 0, opts);
    expect(w.x).toBe(0);
    // Dragging the left edge past the right one stops at the minimum.
    expect(dragCrop(start, "w", 5, 0, opts).w).toBeCloseTo(0.05, 9);
  });

  it("keeps a ratio of pixels, anchored on the opposite side", () => {
    // 4:3 in a 4000 x 3000 frame is the same shape as the frame: w and h in equal fractions.
    const ratio = 4 / 3;
    const o = { ...opts, ratio };
    const a = dragCrop(crop(0.2, 0.2, 0.4, 0.4), "se", 0.2, 0, o);
    expect(a.x).toBeCloseTo(0.2, 9);
    expect(a.y).toBeCloseTo(0.2, 9);
    expect(a.w).toBeCloseTo(0.6, 9);
    expect(a.h).toBeCloseTo(0.6, 9);
    // An edge: the other side follows, centred.
    const b = dragCrop(crop(0.2, 0.2, 0.4, 0.4), "e", 0.2, 0, o);
    expect(b.w).toBeCloseTo(0.6, 9);
    expect(b.h).toBeCloseTo(0.6, 9);
    expect(b.y + b.h / 2).toBeCloseTo(0.4, 9);
    const c = dragCrop(crop(0.2, 0.2, 0.4, 0.4), "s", 0, 0.2, o);
    expect(c.w).toBeCloseTo(0.6, 9);
    expect(c.x + c.w / 2).toBeCloseTo(0.4, 9);
    // At the frame's edge the whole crop stops growing, still in ratio.
    const d = dragCrop(crop(0.5, 0.5, 0.2, 0.2), "se", 5, 5, o);
    expect(d.x + d.w).toBeLessThanOrEqual(1 + 1e-9);
    expect(d.y + d.h).toBeLessThanOrEqual(1 + 1e-9);
    expect((d.w * 4000) / (d.h * 3000)).toBeCloseTo(ratio, 9);
    // A square in a wide frame: equal pixels, unequal fractions.
    const sq = dragCrop(crop(0.1, 0.1, 0.3, 0.4), "se", 0.1, 0, { fw: 4000, fh: 3000, ratio: 1 });
    expect((sq.w * 4000) / (sq.h * 3000)).toBeCloseTo(1, 9);
  });

  it("fits a ratio into the current crop or the frame", () => {
    const sq = fitRatio(null, 1, 4000, 3000);
    expect((sq.w * 4000) / (sq.h * 3000)).toBeCloseTo(1, 9);
    expect(sq.h).toBe(1);
    expect(sq.x + sq.w / 2).toBeCloseTo(0.5, 9);
    const inside = fitRatio(crop(0.1, 0.1, 0.4, 0.4), 16 / 9, 4000, 3000);
    expect(inside.w).toBeCloseTo(0.4, 9);
    expect(inside.x).toBeCloseTo(0.1, 9);
    expect((inside.w * 4000) / (inside.h * 3000)).toBeCloseTo(16 / 9, 9);
  });

  it("offers free, the original's ratio, and the usual ones in both orientations", () => {
    expect(RATIOS.map((r) => r.id)).toEqual(["free", "original", "1:1", "4:3", "3:4", "3:2", "2:3", "16:9", "9:16"]);
    const original = RATIOS.find((r) => r.id === "original")!;
    expect(ratioOf(original, 4000, 3000)).toBeCloseTo(4 / 3, 9);
    expect(ratioOf(RATIOS[0]!, 4000, 3000)).toBeNull();
  });
});
