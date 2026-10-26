import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type AssetList, type MapCluster, type MapResponse } from "../api";
import { AuthProvider } from "../auth";
import type { MapViewport } from "../map/MapCanvas";
import { detail, item } from "../test/fixtures";
import { MapPage } from "./MapPage";

// A stand-in for the MapLibre canvas: the page only sees the props, so the test drives them with buttons.
const canvas = vi.hoisted(() => ({ props: null as null | Record<string, unknown> }));
vi.mock("../map/MapCanvas", () => ({
  default: (props: Record<string, unknown>) => {
    canvas.props = props;
    const p = props as {
      clusters: MapCluster[];
      basemap: { world: boolean };
      onViewport: (v: MapViewport) => void;
      onOpenPhoto: (id: string) => void;
      onUnavailable: (r: string) => void;
    };
    const view = (west: number, east: number, zoom: number) => () =>
      p.onViewport({ bounds: { west, south: 40, east, north: 50 }, zoom });
    return (
      <div data-testid="map" data-basemap={String(p.basemap.world)}>
        <span>{p.clusters.length} markers</span>
        <button onClick={view(-10, 10, 4.6)}>move to Europe</button>
        <button onClick={view(170, 190, 2.1)}>move across the date line</button>
        <button onClick={() => p.onOpenPhoto("id-77")}>marker photo</button>
        <button onClick={() => p.onOpenPhoto("id-1")}>marker photo in list</button>
        <button onClick={() => p.onUnavailable("WebGL is not available")}>break</button>
      </div>
    );
  },
}));

let observers: { cb: (e: { isIntersecting: boolean }[]) => void; off: boolean }[] = [];
// The newest observer that is still connected: the list re-creates it a render after new photos arrive.
const live = () => observers.filter((o) => !o.off).at(-1);
beforeEach(() => {
  observers = [];
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      o: (typeof observers)[number];
      constructor(cb: (e: { isIntersecting: boolean }[]) => void) {
        this.o = { cb, off: false };
        observers.push(this.o);
      }
      observe() {}
      disconnect() {
        this.o.off = true;
      }
      unobserve() {}
    },
  );
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const marker = (over: Partial<MapCluster> = {}): MapCluster => ({
  id: "id-1",
  lat: 48.8,
  lon: 2.3,
  count: 3,
  bounds: [2.2, 48.7, 2.4, 48.9],
  thumbhash: null,
  thumb_url: null,
  ...over,
});
const response = (clusters: MapCluster[], over: Partial<MapResponse> = {}): MapResponse => ({
  clusters,
  total: clusters.reduce((n, c) => n + c.count, 0),
  truncated: false,
  ...over,
});
const page = (n: number[], next: string | null = null): AssetList => ({
  items: n.map((i) => item(i)),
  next_cursor: next,
  next_before: null,
});

function setup(opts: { world?: boolean; overview?: MapResponse } = {}) {
  vi.spyOn(api, "tiles").mockResolvedValue({ world: opts.world ?? true, detail: false });
  const clusters = vi
    .spyOn(api, "mapClusters")
    .mockImplementation(async (bbox) => (bbox === "-180,-90,180,90" ? (opts.overview ?? response([marker()])) : response([marker()])));
  const list = vi.spyOn(api, "listAssets").mockResolvedValue(page([1, 2, 3]));
  render(
    <MemoryRouter initialEntries={["/map"]}>
      <AuthProvider>
        <MapPage />
      </AuthProvider>
    </MemoryRouter>,
  );
  return { clusters, list };
}

const tile = (n: number) => screen.findByRole("button", { name: new RegExp(`^(Open|Select) photo-${n}\\.jpg$`) });

describe("MapPage", () => {
  it("says so when no photo has a location, without starting a map", async () => {
    setup({ overview: response([]) });
    expect(await screen.findByText(/no photos with a location yet/i)).toBeInTheDocument();
    expect(screen.queryByTestId("map")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Places" })).toBeInTheDocument();
  });

  it("has the Places header, credits the data sources and folds the photo panel with its handle", async () => {
    setup();
    expect(await screen.findByTestId("map")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Places" })).toBeInTheDocument();
    const handle = screen.getByRole("button", { name: "Show photos in view" });
    expect(handle).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(handle);
    expect(screen.getByRole("button", { name: "Fold photos in view" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "OpenStreetMap contributors" })).toHaveAttribute("href", expect.stringContaining("openstreetmap.org"));
    expect(screen.getByRole("link", { name: "Protomaps" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "GeoNames" })).toBeInTheDocument();
    expect(screen.queryByText(/no basemap is installed/i)).not.toBeInTheDocument();
  });

  it("falls back to a plain background with a notice when no basemap is installed", async () => {
    setup({ world: false });
    const map = await screen.findByTestId("map");
    expect(map).toHaveAttribute("data-basemap", "false");
    expect(screen.getByText(/no basemap is installed/i)).toBeInTheDocument();
    // Only the places credit remains: no OpenStreetMap/Protomaps data is shown.
    expect(screen.queryByRole("link", { name: "Protomaps" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "GeoNames" })).toBeInTheDocument();
  });

  it("asks for markers and for the photos of the settled viewport, at a whole zoom level", async () => {
    const { clusters, list } = setup();
    await userEvent.click(await screen.findByRole("button", { name: "move to Europe" }));
    await waitFor(() => expect(clusters).toHaveBeenCalledWith("-10,40,10,50", 4, expect.any(AbortSignal)));
    await waitFor(() => expect(list).toHaveBeenCalledWith(expect.objectContaining({ bbox: "-10,40,10,50", limit: 60 })));
    expect(await tile(1)).toBeInTheDocument();
    expect(screen.getByText("3 photos in view")).toBeInTheDocument();
    expect(canvas.props?.clusters).toHaveLength(1);
  });

  it("passes a box across the date line on, for the server to wrap", async () => {
    const { clusters } = setup();
    await userEvent.click(await screen.findByRole("button", { name: "move across the date line" }));
    await waitFor(() => expect(clusters).toHaveBeenCalledWith("170,40,190,50", 2, expect.any(AbortSignal)));
  });

  it("starts the list over for a new area and fetches further pages by keyset cursor", async () => {
    const { list } = setup();
    list.mockImplementation(async (o) => {
      if (o?.bbox === "-10,40,10,50") return o.cursor ? page([4, 5]) : page([1, 2, 3], "c1");
      return page([9]);
    });
    await userEvent.click(await screen.findByRole("button", { name: "move to Europe" }));
    await tile(3);
    // The end of the list comes into view: the next page is requested with the cursor, and appended.
    await waitFor(() => expect(live()).toBeDefined());
    act(() => live()!.cb([{ isIntersecting: true }]));
    expect(await tile(5)).toBeInTheDocument();
    expect(list).toHaveBeenCalledWith(expect.objectContaining({ bbox: "-10,40,10,50", cursor: "c1" }));
    expect(screen.getAllByRole("listitem")).toHaveLength(5);
    // Moving elsewhere replaces the photos.
    await userEvent.click(screen.getByRole("button", { name: "move across the date line" }));
    await tile(9);
    await waitFor(() => expect(screen.queryByRole("button", { name: /photo-1\.jpg$/ })).not.toBeInTheDocument());
  });

  it("opens a tile in the lightbox with the place of the photo, and a marker's photo that is not in the list", async () => {
    setup();
    vi.mocked(api.getAsset).mockImplementation(async (id) => ({
      ...detail(Number(id.split("-")[1])),
      place: { city: "Paris", region: "Île-de-France", country: "France", country_code: "FR" },
    }));
    await userEvent.click(await screen.findByRole("button", { name: "move to Europe" }));
    await userEvent.click(await tile(2));
    const box = await screen.findByRole("dialog", { name: "photo-2.jpg" });
    expect(await within(box).findByText(/Paris, Île-de-France, France/)).toBeInTheDocument();
    await userEvent.click(within(box).getByRole("button", { name: "Close" }));
    await userEvent.click(screen.getByRole("button", { name: "marker photo" }));
    expect(await screen.findByRole("dialog", { name: "photo-77.jpg" })).toBeInTheDocument();
  });

  it("favorites from the lightbox and trashes with undo, then refreshes the markers", async () => {
    const { clusters } = setup();
    const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 1 });
    const trash = vi.spyOn(api, "trashAssets").mockResolvedValue({ count: 1 });
    const restore = vi.spyOn(api, "restoreAssets").mockResolvedValue({ count: 1 });
    await userEvent.click(await screen.findByRole("button", { name: "move to Europe" }));
    await userEvent.click(await tile(1));
    const box = await screen.findByRole("dialog");
    await userEvent.click(within(box).getByRole("button", { name: "Add to favorites" }));
    expect(fav).toHaveBeenCalledWith(["id-1"], true);
    expect(within(box).getByRole("button", { name: "Remove from favorites" })).toBeInTheDocument();
    const before = clusters.mock.calls.length;
    await userEvent.click(within(box).getByRole("button", { name: "Move to trash" }));
    expect(trash).toHaveBeenCalledWith(["id-1"]);
    await waitFor(() => expect(clusters.mock.calls.length).toBeGreaterThan(before));
    expect(screen.queryByRole("button", { name: /photo-1\.jpg$/ })).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(restore).toHaveBeenCalledWith(["id-1"]));
  });

  it("lists the geotagged photos with a notice when the map cannot be drawn", async () => {
    const { list } = setup();
    await userEvent.click(await screen.findByRole("button", { name: "break" }));
    expect(await screen.findByText(/can’t be drawn in this browser \(WebGL is not available\)/)).toBeInTheDocument();
    expect(screen.queryByTestId("map")).not.toBeInTheDocument();
    await waitFor(() => expect(list).toHaveBeenCalledWith(expect.objectContaining({ bbox: "-180,-90,180,90" })));
    expect(await tile(1)).toBeInTheDocument();
  });

  it("shows an error under the header when the overview cannot be loaded", async () => {
    vi.spyOn(api, "tiles").mockResolvedValue({ world: true, detail: false });
    vi.spyOn(api, "mapClusters").mockRejectedValue(new Error("boom"));
    render(
      <MemoryRouter initialEntries={["/map"]}>
        <AuthProvider>
          <MapPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
  });
});
