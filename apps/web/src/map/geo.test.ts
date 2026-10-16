import { describe, expect, it } from "vitest";
import type { MapCluster } from "../api";
import { bboxParam, bubbleSize, clusterTarget, clusterZoom, extent, placeLine, shortCount } from "./geo";

const cluster = (over: Partial<MapCluster> = {}): MapCluster => ({
  id: "a",
  lat: 10,
  lon: 20,
  count: 5,
  bounds: [19, 9, 21, 11],
  thumbhash: null,
  thumb_url: null,
  ...over,
});

describe("bboxParam", () => {
  it("writes west,south,east,north with short numbers", () => {
    expect(bboxParam({ west: -10.123456789, south: 40, east: 10, north: 50.5 })).toBe("-10.12346,40,10,50.5");
  });
  it("keeps longitudes beyond the date line for the server to wrap, and clamps latitudes", () => {
    expect(bboxParam({ west: 170, south: -95, east: 190, north: 95 })).toBe("170,-90,190,90");
  });
  it("sends the whole world when the map shows more than 360 degrees", () => {
    expect(bboxParam({ west: -250, south: -60, east: 250, north: 70 })).toBe("-180,-60,180,70");
  });
});

describe("clusterZoom", () => {
  it("uses whole levels within the API's range", () => {
    expect([clusterZoom(0.9), clusterZoom(5.7), clusterZoom(-2), clusterZoom(40)]).toEqual([0, 5, 0, 22]);
  });
});

describe("placeLine", () => {
  it("joins city, region and country, leaving out a region that repeats the city", () => {
    expect(placeLine({ city: "Paris", region: "Île-de-France", country: "France", country_code: "FR" })).toBe(
      "Paris, Île-de-France, France",
    );
    expect(placeLine({ city: "Singapore", region: "Singapore", country: "Singapore", country_code: "SG" })).toBe(
      "Singapore, Singapore",
    );
    expect(placeLine({ city: "Tromsø", region: null, country: "Norway", country_code: "NO" })).toBe("Tromsø, Norway");
    expect(placeLine(null)).toBe("");
  });
});

describe("clusterTarget", () => {
  it("fits the photos of a spread-out cluster", () => {
    expect(clusterTarget(cluster(), 3)).toEqual({ kind: "bounds", bounds: [[19, 9], [21, 11]] });
  });
  it("zooms in on a cluster whose photos are all at one spot, up to the deepest level", () => {
    const here = cluster({ bounds: [20, 10, 20, 10] });
    expect(clusterTarget(here, 3)).toEqual({ kind: "center", center: [20, 10], zoom: 14 });
    expect(clusterTarget(here, 16)).toEqual({ kind: "center", center: [20, 10], zoom: 18 });
    expect(clusterTarget(here, 18.5, 19)).toEqual({ kind: "center", center: [20, 10], zoom: 19 });
    expect(clusterTarget(cluster({ bounds: null }), 5).kind).toBe("center");
  });
});

describe("extent", () => {
  it("is null for no markers and otherwise covers every photo of every cluster", () => {
    expect(extent([])).toBeNull();
    expect(extent([cluster(), cluster({ count: 1, bounds: null, lat: -5, lon: 40 })])).toEqual({
      west: 19,
      south: -5,
      east: 40,
      north: 11,
    });
  });
});

describe("marker text", () => {
  it("abbreviates big counts and grows the bubble slowly", () => {
    expect([shortCount(7), shortCount(999), shortCount(1200), shortCount(15_400)]).toEqual(["7", "999", "1.2k", "15k"]);
    expect(bubbleSize(2)).toBeLessThan(bubbleSize(100));
    expect(bubbleSize(2)).toBeGreaterThanOrEqual(36);
    expect(bubbleSize(10_000_000)).toBeLessThanOrEqual(64);
  });
});
