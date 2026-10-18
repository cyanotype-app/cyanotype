// The Info panel of the lightbox: the button, the `i` key, the swipe up, Escape and Back, and that paging keeps it open.
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { bareDetails, detail, item, photoDetails } from "../test/fixtures";
import { Lightbox } from "./Lightbox";

vi.mock("../map/PlaceMap", () => ({ default: () => <div data-testid="place-map" /> }));

const items = [1, 2, 3].map((n) => item(n));

function Harness({ onClose = () => {}, loadDetails }: { onClose?: () => void; loadDetails?: (id: string) => Promise<ReturnType<typeof photoDetails>> }) {
  const [i, setI] = useState(0);
  return (
    <MemoryRouter>
      <Lightbox
        items={items}
        index={i}
        onIndexChange={setI}
        onClose={onClose}
        loadDetail={async (id) => detail(Number(id.split("-")[1]))}
        loadDetails={loadDetails}
      />
    </MemoryRouter>
  );
}

const ok = vi.fn(async (id: string) => photoDetails({ id, file: { ...photoDetails().file, name: `${id}.heic` }, files: [{ id, name: `${id}.heic`, format: "HEIC", raw: false, size_bytes: 1 }] }));

beforeEach(async () => {
  // A test that ends with the panel open leaves a history entry whose popstate arrives after the next test has begun.
  await new Promise((r) => setTimeout(r, 20));
  window.history.replaceState(null, "");
  ok.mockClear();
  vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
});
afterEach(() => {
  vi.restoreAllMocks();
});

/** One finger dragging from `from` to `to` (client y) on the photo. */
function drag(el: Element, { from, to, dx = 0, type = "touch", id = 1, ms = 400 }: { from: number; to: number; dx?: number; type?: string; id?: number; ms?: number }) {
  const t0 = performance.now();
  fireEvent.pointerDown(el, { pointerType: type, pointerId: id, clientX: 100, clientY: from, timeStamp: t0 });
  fireEvent.pointerMove(el, { pointerType: type, pointerId: id, clientX: 100 + dx / 2, clientY: (from + to) / 2, timeStamp: t0 + ms / 2 });
  fireEvent.pointerMove(el, { pointerType: type, pointerId: id, clientX: 100 + dx, clientY: to, timeStamp: t0 + ms });
  fireEvent.pointerUp(el, { pointerType: type, pointerId: id, clientX: 100 + dx, clientY: to, timeStamp: t0 + ms });
}
const stage = () => document.querySelector(".lightbox-stage")!;

describe("Lightbox Info panel", () => {
  it("offers Info only when the details can be read", () => {
    const { unmount } = render(<Harness />);
    expect(screen.queryByRole("button", { name: "Info" })).not.toBeInTheDocument();
    unmount();
    render(<Harness loadDetails={ok} />);
    expect(screen.getByRole("button", { name: "Info" })).toHaveAttribute("aria-expanded", "false");
  });

  it("opens from the button, shows the photo's details, and gives focus back on close", async () => {
    const user = userEvent.setup();
    render(<Harness loadDetails={ok} />);
    const button = screen.getByRole("button", { name: "Info" });
    await user.click(button);
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(button).toHaveAttribute("aria-controls", "details-panel");
    expect(await screen.findByText("Tuesday, July 4, 2023")).toBeInTheDocument();
    expect(ok).toHaveBeenCalledWith("id-1");
    expect(screen.getByRole("button", { name: "Close info" })).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "Close info" }));
    await waitFor(() => expect(screen.queryByRole("complementary")).not.toBeInTheDocument());
    expect(button).toHaveFocus();
    expect(button).toHaveAttribute("aria-expanded", "false");
  });

  it("toggles with the i key, but not while typing", async () => {
    const user = userEvent.setup();
    render(<Harness loadDetails={ok} />);
    await user.keyboard("i");
    expect(await screen.findByRole("complementary", { name: "Photo details" })).toBeInTheDocument();
    await user.keyboard("i");
    await waitFor(() => expect(screen.queryByRole("complementary")).not.toBeInTheDocument());
  });

  it("closes the panel on the first Escape and the lightbox on the second", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Harness loadDetails={ok} onClose={onClose} />);
    await user.keyboard("i");
    await screen.findByRole("complementary");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("complementary")).not.toBeInTheDocument());
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("closes the panel with the browser's Back button, staying on the page", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Harness loadDetails={ok} onClose={onClose} />);

    await user.keyboard("i");
    await screen.findByRole("complementary");
    expect((window.history.state as { cyInfo?: boolean } | null)?.cyInfo).toBe(true);
    act(() => window.history.back());
    await waitFor(() => expect(screen.queryByRole("complementary")).not.toBeInTheDocument());
    expect(onClose).not.toHaveBeenCalled();
  });

  it("takes its history entry off when the lightbox closes with the panel open", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Harness loadDetails={ok} onClose={onClose} />);
    await user.keyboard("i");
    await screen.findByRole("complementary");
    expect((window.history.state as { cyInfo?: boolean } | null)?.cyInfo).toBe(true);
    await user.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect((window.history.state as { cyInfo?: boolean } | null)?.cyInfo).toBeFalsy();
  });

  it("stays open while paging and reads the next photo's details", async () => {
    const user = userEvent.setup();
    render(<Harness loadDetails={ok} />);
    await user.keyboard("i");
    await screen.findByRole("complementary");
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(ok).toHaveBeenCalledWith("id-2"));
    expect(screen.getByRole("complementary", { name: "Photo details" })).toBeInTheDocument();
    expect(await screen.findByText("id-2.heic")).toBeInTheDocument();
  });

  it("shows a retry when the details fail, and reads them again", async () => {
    const user = userEvent.setup();
    const load = vi.fn().mockRejectedValueOnce(new Error("boom")).mockImplementation(async () => bareDetails());
    render(<Harness loadDetails={load} />);
    await user.keyboard("i");
    await user.click(await screen.findByRole("button", { name: "Try again" }));
    expect(await screen.findByText("scan.png")).toBeInTheDocument();
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("does not take i for itself when a dialog over the lightbox has the keyboard", async () => {
    const user = userEvent.setup();
    render(
      <>
        <Harness loadDetails={ok} />
        <div role="dialog" aria-label="Over the lightbox">
          <button type="button">inside</button>
        </div>
      </>,
    );
    screen.getByRole("button", { name: "inside" }).focus();
    await user.keyboard("i");
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  });
});

describe("swipe up", () => {
  beforeEach(() => {
    // jsdom has no layout; a pinch-zoomed page is the one case that reads the viewport.
    Object.defineProperty(window, "visualViewport", { configurable: true, value: { scale: 1 } });
  });

  it("opens the panel on a clear upward drag of one finger", async () => {
    render(<Harness loadDetails={ok} />);
    drag(stage(), { from: 500, to: 380 });
    expect(await screen.findByRole("complementary", { name: "Photo details" })).toBeInTheDocument();
  });

  it("opens on a short quick flick", async () => {
    render(<Harness loadDetails={ok} />);
    drag(stage(), { from: 500, to: 460, ms: 120 });
    expect(await screen.findByRole("complementary")).toBeInTheDocument();
  });

  it("leaves a mouse, a slow short drag, a downward drag and a mostly sideways one alone", () => {
    render(<Harness loadDetails={ok} />);
    drag(stage(), { from: 500, to: 380, type: "mouse" });
    drag(stage(), { from: 500, to: 470, ms: 900 });
    drag(stage(), { from: 380, to: 500 });
    drag(stage(), { from: 500, to: 440, dx: 200 }); // paging, not the details
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  });

  it("ignores a pinch (a second finger) and a page that is zoomed in", () => {
    render(<Harness loadDetails={ok} />);
    const el = stage();
    fireEvent.pointerDown(el, { pointerType: "touch", pointerId: 2, clientX: 200, clientY: 500 });
    drag(el, { from: 500, to: 380, id: 1 });
    fireEvent.pointerUp(el, { pointerType: "touch", pointerId: 2 });
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();

    Object.defineProperty(window, "visualViewport", { configurable: true, value: { scale: 2.5 } });
    drag(el, { from: 500, to: 380 });
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  });

  it("does not start on a button (the arrows)", () => {
    render(<Harness loadDetails={ok} />);
    drag(screen.getByRole("button", { name: "Next photo" }), { from: 500, to: 380 });
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  });

  it("puts the sheet away with a downward drag on its header", async () => {
    render(<Harness loadDetails={ok} />);
    drag(stage(), { from: 500, to: 380 });
    await screen.findByRole("complementary");
    drag(document.querySelector(".details-head")!, { from: 300, to: 420 });
    await waitFor(() => expect(screen.queryByRole("complementary")).not.toBeInTheDocument());
  });
});
