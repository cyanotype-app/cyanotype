import { describe, expect, it } from "vitest";
import type { AssetListItem } from "./api";
import {
  alone,
  canStack,
  expandIds,
  expandTarget,
  hiddenInStacks,
  photosIn,
  positionOf,
  selectionNote,
  stackLabel,
  step,
  substitute,
  tileIds,
} from "./stacks";
import { item } from "./test/fixtures";

const stacked = (n: number, others: number[]): AssetListItem => ({
  ...item(n),
  stack: { size: others.length + 1, members: [`id-${n}`, ...others.map((o) => `id-${o}`)] },
});
const ids = (xs: readonly { id: string }[]) => xs.map((x) => x.id);

describe("a stack's tile", () => {
  const tiles = [item(1), stacked(2, [3, 4]), item(5)];

  it("stands for all of its photos while collapsed, and for itself otherwise", () => {
    expect(tileIds(tiles[1]!)).toEqual(["id-2", "id-3", "id-4"]);
    expect(tileIds(tiles[0]!)).toEqual(["id-1"]);
    // Expanded lists carry the size but no member ids: every photo is a tile of its own.
    expect(tileIds({ ...item(6), stack: { size: 3 } })).toEqual(["id-6"]);
    expect([photosIn(tiles[1]!), photosIn(tiles[0]!)]).toEqual([3, 1]);
  });

  it("expands a selection to the photos behind it, once each", () => {
    expect(expandIds(tiles, ["id-1", "id-2"])).toEqual(["id-1", "id-2", "id-3", "id-4"]);
    expect(expandIds(tiles, ["id-2", "id-3"])).toEqual(["id-2", "id-3", "id-4"]);
    expect(expandIds(tiles, ["id-9"])).toEqual(["id-9"]);
  });

  it("expands the exceptions of a select-all too", () => {
    expect(expandTarget(["id-2"], tiles)).toEqual(["id-2", "id-3", "id-4"]);
    expect(expandTarget({ scope: "library", except: ["id-2", "id-5"] }, tiles)).toEqual({
      scope: "library",
      except: ["id-2", "id-3", "id-4", "id-5"],
    });
  });

  it("counts what a selection hides and says so", () => {
    expect(hiddenInStacks(tiles, new Set(["id-1", "id-2"]))).toBe(2);
    expect(hiddenInStacks(tiles, new Set(["id-1"]))).toBe(0);
    expect(selectionNote(2)).toBe("plus 2 more in stacks");
    expect(selectionNote(0)).toBeNull();
  });

  it("can be stacked with another photo, or alone when it is already a stack, never as a select-all", () => {
    expect(canStack(tiles, new Set(["id-1", "id-5"]), false)).toBe(true);
    expect(canStack(tiles, new Set(["id-1"]), false)).toBe(false);
    expect(canStack(tiles, new Set(["id-2"]), false)).toBe(true);
    expect(canStack(tiles, new Set(["id-1", "id-5"]), true)).toBe(false);
    expect(canStack(tiles, new Set(["id-1", "id-2"]), false, 3)).toBe(false);
  });

  it("is a photo on its own once the viewer shows one of its photos", () => {
    expect(alone(tiles[1]!).stack).toBeUndefined();
    expect(alone(tiles[1]!).id).toBe("id-2");
    expect(alone(tiles[0]!)).toBe(tiles[0]);
  });
});

describe("stepping through a stack in the viewer", () => {
  const members = [item(1), item(2), item(3)];

  it("finds the photo on show and stops at both ends", () => {
    expect(positionOf(members, "id-3")).toBe(2);
    expect(positionOf(members, "id-9")).toBe(0);
    expect(positionOf(members, null)).toBe(0);
    expect(step(0, -1, 3)).toBe(0);
    expect(step(1, 1, 3)).toBe(2);
    expect(step(2, 1, 3)).toBe(2);
    expect(step(0, 1, 0)).toBe(0);
  });

  it("swaps the member in for the timeline's tile without disturbing the photos around it", () => {
    const run = [item(10), stacked(11, [12]), item(13)];
    const shown = substitute(run, 1, members[2]);
    expect(ids(shown)).toEqual(["id-10", "id-3", "id-13"]);
    expect(ids(run)).toEqual(["id-10", "id-11", "id-13"]);
    expect(ids(substitute(run, 1, undefined))).toEqual(["id-10", "id-11", "id-13"]);
    expect(ids(substitute(run, 5, members[0]))).toEqual(["id-10", "id-11", "id-13"]);
  });

  it("labels the stack", () => {
    expect(stackLabel(5)).toBe("Stack of 5 photos");
  });
});
