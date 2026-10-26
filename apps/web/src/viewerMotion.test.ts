import { describe, expect, it } from "vitest";
import { liftOnto } from "./viewerMotion";

const box = { left: 0, top: 0, width: 400, height: 800 };

describe("liftOnto", () => {
  it("covers a square tile with a landscape photo, centres meeting, and clips to the tile", () => {
    const tile = { left: 20, top: 100, width: 100, height: 100 };
    const l = liftOnto(tile, box, 3 / 2);
    // The photo fits the box at 400 x 266.7; covering 100 x 100 needs 100 / 266.7 of it.
    expect(l.scale).toBeCloseTo(100 / (400 / 1.5), 5);
    expect(l.x).toBeCloseTo(70 - 200, 5);
    expect(l.y).toBeCloseTo(150 - 400, 5);
    // The clip is the tile seen from inside the box: centred, one tile wide divided by the scale.
    const half = 50 / l.scale;
    expect(l.clip).toBe(`inset(${400 - half}px ${200 - half}px ${400 - half}px ${200 - half}px round ${6 / l.scale}px)`);
  });

  it("covers a tall tile with a portrait photo by its width", () => {
    const tile = { left: 0, top: 0, width: 100, height: 150 };
    const l = liftOnto(tile, { left: 0, top: 0, width: 400, height: 400 }, 2 / 3);
    // Contained in a square box: 266.7 x 400; the tile's 100 x 150 is exactly that shape.
    expect(l.scale).toBeCloseTo(100 / (400 * (2 / 3)), 5);
  });

  it("measures positions against the box's own place on the page (a side panel, a notch)", () => {
    const tile = { left: 300, top: 50, width: 80, height: 80 };
    const l = liftOnto(tile, { left: 100, top: 0, width: 600, height: 400 }, 1);
    expect(l.x).toBeCloseTo(340 - 400, 5);
    expect(l.y).toBeCloseTo(90 - 200, 5);
  });
});
