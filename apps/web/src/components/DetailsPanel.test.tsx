import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { bareDetails, photoDetails } from "../test/fixtures";
import { DetailsPanel, type DetailsState } from "./DetailsPanel";

// The small map needs WebGL: a stand-in records that it was asked for and with what.
vi.mock("../map/PlaceMap", () => ({
  default: (p: { lat: number; lon: number }) => <div data-testid="place-map">{`${p.lat},${p.lon}`}</div>,
}));

function Where() {
  const l = useLocation();
  return <p data-testid="where">{l.pathname + l.search}</p>;
}

function setup(state: DetailsState) {
  const props = {
    panelId: "details-panel",
    state,
    onRetry: vi.fn(),
    onClose: vi.fn(),
    onLeave: vi.fn(() => Promise.resolve()),
    onNavigate: vi.fn(),
  };
  render(
    <MemoryRouter initialEntries={["/library"]}>
      <Routes>
        <Route
          path="*"
          element={
            <>
              <DetailsPanel {...props} />
              <Where />
            </>
          }
        />
      </Routes>
    </MemoryRouter>,
  );
  return props;
}

beforeEach(() => {
  vi.spyOn(api, "tiles").mockResolvedValue({ world: true, detail: false });
});

describe("DetailsPanel", () => {
  it("shows every row of a fully described photo", async () => {
    setup({ status: "ready", details: photoDetails() });
    const panel = screen.getByRole("complementary", { name: "Photo details" });
    expect(within(panel).getByText("Tuesday, July 4, 2023")).toBeInTheDocument();
    expect(panel).toHaveTextContent("1:05 PM · UTC+02:00");
    expect(within(panel).getByText("Lisbon, Portugal")).toBeInTheDocument();
    expect(within(panel).getByText("38.7223° N, 9.1393° W")).toBeInTheDocument();
    expect(within(panel).getByText("Altitude 38 m above sea level")).toBeInTheDocument();
    expect(within(panel).getByText("Facing 132° SE")).toBeInTheDocument();
    expect(await within(panel).findByTestId("place-map")).toHaveTextContent("38.7223,-9.1393");
    expect(within(panel).getByText("Apple iPhone 15")).toBeInTheDocument();
    expect(within(panel).getByText("iPhone 15 back camera 6.765mm f/1.78")).toBeInTheDocument();
    expect(within(panel).getByText("ISO 100 · 6.8 mm · ƒ/1.8 · 1/120 s · +0.3 ev")).toBeInTheDocument();
    expect(within(panel).getByText("24 mm")).toBeInTheDocument();
    for (const t of ["Did not fire", "Auto", "Pattern", "Normal program"]) expect(within(panel).getByText(t)).toBeInTheDocument();
    expect(within(panel).getByText("4032 × 3024 · 12 MP")).toBeInTheDocument();
    expect(within(panel).getByText("HEIC · 2.4 MB")).toBeInTheDocument();
    expect(within(panel).getByText("Uncalibrated")).toBeInTheDocument();
    expect(within(panel).getByText("17.4.1")).toBeInTheDocument();
    expect(within(panel).getByRole("link", { name: "Portugal" })).toHaveAttribute("href", "/albums/album-1");
    expect(within(panel).getByText("1 of 8 in a burst")).toBeInTheDocument();
  });

  it("hides the rows a photo has nothing for, and says when the zone and capture time are missing", () => {
    setup({ status: "ready", details: bareDetails() });
    const panel = screen.getByRole("complementary", { name: "Photo details" });
    for (const label of ["Location", "Camera", "Lens", "Exposure", "35 mm equivalent", "Flash", "Albums", "Burst", "Resolution", "Software", "Colour space"]) {
      expect(within(panel).queryByText(label)).not.toBeInTheDocument();
    }
    expect(within(panel).queryByTestId("place-map")).not.toBeInTheDocument();
    expect(within(panel).getByText("Added")).toBeInTheDocument();
    expect(within(panel).getByText("Time zone unknown")).toBeInTheDocument();
    expect(within(panel).getByText("scan.png")).toBeInTheDocument();
  });

  it("lists both files of a RAW + JPEG pair", () => {
    const d = photoDetails({
      files: [
        { id: "id-1", name: "DSC_1.JPG", format: "JPEG", raw: false, size_bytes: 6_000_000 },
        { id: "raw-1", name: "DSC_1.NEF", format: "NEF", raw: true, size_bytes: 30_000_000 },
      ],
    });
    setup({ status: "ready", details: d });
    expect(screen.getByText("Files")).toBeInTheDocument();
    expect(screen.getByText("DSC_1.JPG")).toBeInTheDocument();
    expect(screen.getByText("NEF (RAW) · 29 MB")).toBeInTheDocument();
  });

  it("links the camera, lens, place and date to the search and the timeline", async () => {
    const user = userEvent.setup();
    const p = setup({ status: "ready", details: photoDetails() });
    let last = screen.getByTestId("where").textContent;
    const go = async (name: string | RegExp) => {
      await user.click(screen.getByRole("link", { name }));
      await waitFor(() => expect(p.onNavigate).toHaveBeenCalled());
      // The router commits the new location a moment after the click handler ran: wait for it, or read the previous one.
      await waitFor(() => expect(screen.getByTestId("where").textContent).not.toBe(last));
      const where = screen.getByTestId("where").textContent;
      last = where;
      p.onNavigate.mockClear();
      return where;
    };
    expect(await go("Apple iPhone 15")).toBe("/search?camera=Apple+iPhone+15");
    expect(await go("iPhone 15 back camera 6.765mm f/1.78")).toBe("/search?lens=iPhone+15+back+camera+6.765mm+f%2F1.78");
    expect(await go("Lisbon, Portugal")).toBe("/search?place=Lisbon");
    expect(await go("Tuesday, July 4, 2023")).toBe("/photos?day=2023-07-04");
    expect(await go("Open the map at this photo")).toBe("/map?lat=38.7223&lon=-9.1393");
    // Each followed link took the panel's history entry off first.
    expect(p.onLeave).toHaveBeenCalledTimes(5);
  });

  it("closes from its button, and offers a retry when the details could not be read", async () => {
    const user = userEvent.setup();
    const p = setup({ status: "error" });
    expect(screen.getByRole("alert")).toHaveTextContent("Could not load the details.");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(p.onRetry).toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Close info" }));
    expect(p.onClose).toHaveBeenCalled();
  });

  it("moves focus into the panel when it opens, and says it is loading", () => {
    setup({ status: "loading" });
    expect(screen.getByRole("status")).toHaveTextContent("Loading details");
    expect(screen.getByRole("button", { name: "Close info" })).toHaveFocus();
  });
});
