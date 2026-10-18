import { render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// MapLibre needs WebGL, which jsdom lacks: a recording stand-in checks what the small map asks of it.
const h = vi.hoisted(() => {
  const state = { maps: [] as FakeMap[], markers: [] as FakeMarker[], fail: false };
  class FakeMap {
    calls: string[] = [];
    constructor(public opts: Record<string, unknown>) {
      if (state.fail) throw new Error("Failed to initialize WebGL");
      state.maps.push(this);
    }
    setStyle() {
      this.calls.push("setStyle");
    }
    remove() {
      this.calls.push("remove");
    }
  }
  class FakeMarker {
    lngLat: [number, number] = [0, 0];
    constructor(public opts: { element: HTMLElement; anchor: string }) {
      state.markers.push(this);
    }
    setLngLat(l: [number, number]) {
      this.lngLat = l;
      return this;
    }
    addTo() {
      return this;
    }
  }
  return { state, FakeMap, FakeMarker };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));
vi.mock("maplibre-gl", () => ({ Map: h.FakeMap, Marker: h.FakeMarker, setWorkerUrl: () => undefined, addProtocol: () => undefined }));
vi.mock("maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url", () => ({ default: "/static/worker.js" }));
vi.mock("pmtiles", () => ({ Protocol: class { tile = () => undefined } }));

import PlaceMap from "./PlaceMap";

const base = { lat: 38.7223, lon: -9.1393, basemap: { world: true, detail: false }, dark: false };

beforeEach(() => {
  h.state.maps = [];
  h.state.markers = [];
  h.state.fail = false;
});
afterEach(() => {
  document.body.innerHTML = "";
});

describe("PlaceMap", () => {
  it("draws a non-interactive map centred on the photo, with a pin on it", async () => {
    render(<PlaceMap {...base} />);
    await waitFor(() => expect(h.state.maps).toHaveLength(1));
    const map = h.state.maps[0]!;
    expect(map.opts.interactive).toBe(false);
    expect(map.opts.center).toEqual([-9.1393, 38.7223]);
    expect(map.opts.attributionControl).toBe(false);
    expect((map.opts.style as { sources: object }).sources).toHaveProperty("world");
    expect(h.state.markers[0]!.lngLat).toEqual([-9.1393, 38.7223]);
    expect(h.state.markers[0]!.opts.element.className).toBe("place-pin");
  });

  it("restyles for dark mode, and removes the map when unmounted", async () => {
    const { rerender, unmount } = render(<PlaceMap {...base} />);
    await waitFor(() => expect(h.state.maps).toHaveLength(1));
    rerender(<PlaceMap {...base} dark />);
    expect(h.state.maps[0]!.calls).toContain("setStyle");
    unmount();
    expect(h.state.maps[0]!.calls).toContain("remove");
  });

  it("tells the panel when the map cannot start", async () => {
    h.state.fail = true;
    const onUnavailable = vi.fn();
    render(<PlaceMap {...base} onUnavailable={onUnavailable} />);
    await waitFor(() => expect(onUnavailable).toHaveBeenCalled());
  });
});
