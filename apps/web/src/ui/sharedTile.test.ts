import { afterEach, describe, expect, it } from "vitest";
import { tileRect } from "./sharedTile";

afterEach(() => {
  document.body.innerHTML = "";
});

function tile(id: string, rect: Partial<DOMRect>) {
  const el = document.createElement("div");
  el.setAttribute("data-asset-id", id);
  el.getBoundingClientRect = () => ({ left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0, toJSON: () => ({}), ...rect }) as DOMRect;
  document.body.append(el);
  return el;
}

describe("tileRect", () => {
  it("is null when the photo has no tile on the page", () => {
    expect(tileRect("a1")).toBeNull();
  });

  it("is the tile's box on screen", () => {
    tile("a1", { left: 10, top: 20, width: 100, height: 90 });
    tile("a2", { left: 200, top: 20, width: 100, height: 90 });
    expect(tileRect("a1")).toMatchObject({ left: 10, top: 20, width: 100, height: 90 });
    expect(tileRect("a2")?.left).toBe(200);
  });

  it("ignores a tile that takes no room (not laid out)", () => {
    tile("a1", { width: 0, height: 0 });
    expect(tileRect("a1")).toBeNull();
  });

  it("takes an id with quotes without breaking the lookup", () => {
    expect(tileRect('a"1')).toBeNull();
  });
});
