import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type Trip } from "../api";
import { AuthProvider } from "../auth";
import { item } from "../test/fixtures";
import { parseTripId, TripPage } from "./TripPage";

beforeEach(() => {
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    },
  );
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const trip: Trip = {
  id: "2026-10-12_2026-10-19",
  from: "2026-10-12",
  to: "2026-10-19",
  ongoing: false,
  title: "New York",
  date_label: "October 2026",
  place: { name: "New York", level: "city", city: "New York", country: "United States", country_code: "US" },
  photos: 124,
  cover_id: "id-1",
  cover: item(1),
};

function open(state?: { trip: Trip }, path = `/trips/${trip.id}`) {
  render(
    <MemoryRouter initialEntries={[{ pathname: path, state }]}>
      <AuthProvider>
        <Routes>
          <Route path="/trips/:id" element={<TripPage />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe("parseTripId", () => {
  it("reads <from>_<to> and nothing else", () => {
    expect(parseTripId("2026-10-12_2026-10-19")).toEqual({ from: "2026-10-12", to: "2026-10-19" });
    expect(parseTripId("2026-10-19_2026-10-12")).toBeNull();
    expect(parseTripId("2026-10-12")).toBeNull();
    expect(parseTripId("x_y")).toBeNull();
  });
});

describe("Trip page", () => {
  it("shows the trip's photos for its dates, under its place, dates and count, with a way back", async () => {
    const list = vi.spyOn(api, "listAssets").mockResolvedValue({ items: [item(1), item(2)], next_cursor: null, next_before: null });
    open({ trip });
    expect(screen.getByRole("heading", { level: 1, name: "New York" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Open photo-2.jpg" })).toBeInTheDocument();
    expect(list).toHaveBeenCalledWith(expect.objectContaining({ from: "2026-10-12", to: "2026-10-19", memories: true }));
    expect(screen.getByText("Oct 12 – 19, 2026 · 124 photos")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Collections" })).toHaveAttribute("href", "/");
  });

  it("looks the trip up when opened from a link, and counts the photos itself if it is gone", async () => {
    const trips = vi.spyOn(api, "trips").mockResolvedValue({ enabled: true, trips: [trip], next_before: null });
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [item(1)], next_cursor: null, next_before: null });
    open();
    expect(await screen.findByRole("heading", { level: 1, name: "New York" })).toBeInTheDocument();
    expect(trips).toHaveBeenCalledWith(expect.objectContaining({ limit: 1, before: "2026-10-20" }));
  });

  it("says so when the trip has no photos left", async () => {
    vi.spyOn(api, "trips").mockResolvedValue({ enabled: true, trips: [], next_before: null });
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [], next_cursor: null, next_before: null });
    open();
    expect(await screen.findByRole("heading", { level: 2, name: "No photos here" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("Oct 12 – 19, 2026 · 0 photos")).toBeInTheDocument());
  });

  it("rejects an id that is not a trip", () => {
    open(undefined, "/trips/nope");
    expect(screen.getByRole("heading", { name: "That isn’t a trip" })).toBeInTheDocument();
  });
});
