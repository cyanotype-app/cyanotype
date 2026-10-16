import { act, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MapCluster } from "../api";

// MapLibre needs WebGL, which jsdom lacks: a recording stand-in checks what the component asks of it.
const h = vi.hoisted(() => {
  const state = {
    maps: [] as FakeMap[],
    markers: [] as FakeMarker[],
    protocols: [] as string[],
    fail: false,
  };
  class FakeMap {
    handlers: Record<string, (() => void)[]> = {};
    calls: [string, unknown][] = [];
    touchZoomRotate = { disableRotation: () => undefined };
    constructor(public opts: Record<string, unknown>) {
      if (state.fail) throw new Error("Failed to initialize WebGL");
      state.maps.push(this);
    }
    on(e: string, f: () => void) {
      (this.handlers[e] ??= []).push(f);
    }
    once(e: string, f: () => void) {
      this.on(e, f);
    }
    fire(e: string) {
      this.handlers[e]?.forEach((f) => f());
    }
    addControl() {}
    getBounds() {
      return { getWest: () => -20, getSouth: () => 30, getEast: () => 40, getNorth: () => 60 };
    }
    getZoom() {
      return 3.4;
    }
    fitBounds(b: unknown, o: unknown) {
      this.calls.push(["fitBounds", [b, o]]);
    }
    easeTo(o: unknown) {
      this.calls.push(["easeTo", o]);
    }
    jumpTo(o: unknown) {
      this.calls.push(["jumpTo", o]);
    }
    setStyle(s: unknown) {
      this.calls.push(["setStyle", s]);
    }
    remove() {
      this.calls.push(["remove", null]);
    }
  }
  class FakeMarker {
    removed = false;
    lngLat: [number, number] = [0, 0];
    constructor(public opts: { element: HTMLElement }) {
      state.markers.push(this);
    }
    setLngLat(l: [number, number]) {
      this.lngLat = l;
      return this;
    }
    addTo() {
      document.body.appendChild(this.opts.element);
      return this;
    }
    remove() {
      this.removed = true;
      this.opts.element.remove();
    }
  }
  return { state, FakeMap, FakeMarker };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));
vi.mock("maplibre-gl", () => ({
  Map: h.FakeMap,
  Marker: h.FakeMarker,
  NavigationControl: class {},
  setWorkerUrl: () => undefined,
  addProtocol: (name: string) => h.state.protocols.push(name),
}));
vi.mock("maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url", () => ({ default: "/static/worker.js" }));
vi.mock("pmtiles", () => ({ Protocol: class { tile = () => undefined } }));

import MapCanvas from "./MapCanvas";

const cl = (over: Partial<MapCluster>): MapCluster => ({
  id: "a",
  lat: 10,
  lon: 20,
  count: 1,
  bounds: null,
  thumbhash: null,
  thumb_url: "https://s3.test/t.webp",
  ...over,
});
const base = { basemap: { world: true, detail: false }, dark: false, fitTo: null };

beforeEach(() => {
  h.state.maps = [];
  h.state.markers = [];
  h.state.fail = false;
});
afterEach(() => {
  document.body.innerHTML = "";
});

describe("MapCanvas", () => {
  it("starts MapLibre with the pmtiles protocol, reports the viewport, and draws a marker per cluster", async () => {
    const onViewport = vi.fn();
    const onOpenPhoto = vi.fn();
    const clusters = [cl({ id: "p1" }), cl({ id: "c1", count: 12, lat: 1, lon: 2, bounds: [0, 0, 4, 2] })];
    render(<MapCanvas {...base} clusters={clusters} onViewport={onViewport} onOpenPhoto={onOpenPhoto} onUnavailable={vi.fn()} />);
    await waitFor(() => expect(h.state.maps).toHaveLength(1));
    const map = h.state.maps[0]!;
    expect(h.state.protocols).toEqual(["pmtiles"]);
    expect((map.opts.style as { sources: object }).sources).toHaveProperty("world");
    expect(map.opts.attributionControl).toBe(false);
    act(() => map.fire("moveend"));
    expect(onViewport).toHaveBeenCalledWith({ bounds: { west: -20, south: 30, east: 40, north: 60 }, zoom: 3.4 });
    expect(h.state.markers).toHaveLength(2);
    // A photo is a thumbnail button that opens it; a cluster is a count bubble that fits its photos.
    const photo = document.querySelector<HTMLElement>(".map-thumb")!;
    expect(photo.querySelector("img")).toHaveAttribute("src", "https://s3.test/t.webp");
    photo.click();
    expect(onOpenPhoto).toHaveBeenCalledWith("p1");
    const bubble = document.querySelector<HTMLElement>(".map-bubble")!;
    expect(bubble.textContent).toBe("12");
    bubble.click();
    expect(map.calls.find(([n]) => n === "fitBounds")?.[1]).toEqual([
      [[0, 0], [4, 2]],
      expect.objectContaining({ maxZoom: 19 }),
    ]);
    // Panning ends: the viewport is reported again.
    onViewport.mockClear();
    act(() => map.fire("moveend"));
    expect(onViewport).toHaveBeenCalledTimes(1);
  });

  it("zooms toward a cluster whose photos share one spot, and keeps unchanged markers when clusters update", async () => {
    const same = cl({ id: "c2", count: 4, lat: 5, lon: 6, bounds: [6, 5, 6, 5] });
    const { rerender } = render(
      <MapCanvas {...base} clusters={[same]} onViewport={vi.fn()} onOpenPhoto={vi.fn()} onUnavailable={vi.fn()} />,
    );
    await waitFor(() => expect(h.state.maps).toHaveLength(1));
    
    document.querySelector<HTMLElement>(".map-bubble")!.click();
    expect(h.state.maps[0]!.calls.find(([n]) => n === "easeTo")?.[1]).toMatchObject({ center: [6, 5], zoom: 14 });
    const first = h.state.markers[0]!;
    rerender(
      <MapCanvas {...base} clusters={[same, cl({ id: "p2", lat: 1, lon: 1 })]} onViewport={vi.fn()} onOpenPhoto={vi.fn()} onUnavailable={vi.fn()} />,
    );
    expect(h.state.markers).toHaveLength(2);
    expect(first.removed).toBe(false);
    rerender(<MapCanvas {...base} clusters={[]} onViewport={vi.fn()} onOpenPhoto={vi.fn()} onUnavailable={vi.fn()} />);
    expect(h.state.markers.every((m) => m.removed)).toBe(true);
  });

  it("restyles for dark mode and for a basemap that was installed meanwhile", async () => {
    const { rerender } = render(
      <MapCanvas {...base} basemap={{ world: false, detail: false }} clusters={[]} onViewport={vi.fn()} onOpenPhoto={vi.fn()} onUnavailable={vi.fn()} />,
    );
    await waitFor(() => expect(h.state.maps).toHaveLength(1));
    const style = h.state.maps[0]!.opts.style as { sources: object; layers: { id: string }[] };
    expect(style.sources).toEqual({});
    expect(style.layers.map((l) => l.id)).toEqual(["plain-background"]);
    rerender(<MapCanvas {...base} dark clusters={[]} onViewport={vi.fn()} onOpenPhoto={vi.fn()} onUnavailable={vi.fn()} />);
    expect(h.state.maps[0]!.calls.filter(([n]) => n === "setStyle")).toHaveLength(1);
  });

  it("reports when the map cannot start", async () => {
    h.state.fail = true;
    const onUnavailable = vi.fn();
    render(<MapCanvas {...base} clusters={[]} onViewport={vi.fn()} onOpenPhoto={vi.fn()} onUnavailable={onUnavailable} />);
    await waitFor(() => expect(onUnavailable).toHaveBeenCalledWith("Failed to initialize WebGL"));
  });

  it("removes the map and its markers when unmounted", async () => {
    const { unmount } = render(
      <MapCanvas {...base} clusters={[cl({})]} onViewport={vi.fn()} onOpenPhoto={vi.fn()} onUnavailable={vi.fn()} />,
    );
    await waitFor(() => expect(h.state.maps).toHaveLength(1));
    
    unmount();
    expect(h.state.maps[0]!.calls.some(([n]) => n === "remove")).toBe(true);
    expect(h.state.markers[0]!.removed).toBe(true);
  });
});
