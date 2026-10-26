import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { buildIndex, buildLayout, offsetOf } from "../timeline";
import { Scrubber } from "./Scrubber";

// Six years, a month of 30 photos each: 72 months.
const layoutOf = (months: number) =>
  buildLayout(
    buildIndex(
      Array.from({ length: months }, (_, k) => ({
        day: `${2026 - Math.floor(k / 12)}-${String(12 - (k % 12)).padStart(2, "0")}-15`,
        count: 30,
      })),
    ),
    { cols: 4, tile: 100, gap: 2 },
  );

const TRACK = { top: 100, height: 630 }; // the handle is 44 px tall: its travel is 586
let scrollTo: ReturnType<typeof vi.fn>;

beforeEach(() => {
  scrollTo = vi.fn();
  vi.stubGlobal("scrollTo", scrollTo);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    return this.hasAttribute("data-scrubber")
      ? ({ ...TRACK, bottom: TRACK.top + TRACK.height, left: 0, right: 26, width: 26, x: 0, y: TRACK.top, toJSON() {} } as DOMRect)
      : new DOMRect(0, 0, 0, 0);
  });
  Object.defineProperty(window, "scrollY", { value: 0, configurable: true });
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Scrubber", () => {
  it("lists the years of the library on the rail", () => {
    render(<Scrubber layout={layoutOf(72)} scrollMargin={0} />);
    const years = Array.from(document.querySelectorAll("[data-year]")).map((e) => e.textContent);
    expect(years).toEqual(["2026", "2025", "2024", "2023", "2022", "2021"]);
    expect(screen.getByRole("slider", { name: /by date/i })).toHaveAttribute("aria-valuetext", "December 2026");
  });

  it("is not shown for a library that fits on a couple of screens", () => {
    render(<Scrubber layout={layoutOf(2)} scrollMargin={0} />);
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();
    render(<Scrubber layout={buildLayout(buildIndex([{ day: "2026-01-01", count: 500 }]), { cols: 4, tile: 100, gap: 2 })} scrollMargin={0} />);
    expect(screen.queryByRole("slider")).not.toBeInTheDocument(); // one month: nothing to jump between
  });

  it("scrolls the page while the handle is dragged and names the month under it", () => {
    const layout = layoutOf(72);
    render(<Scrubber layout={layout} scrollMargin={200} />);
    const rail = document.querySelector("[data-scrubber]")!;
    const handle = screen.getByRole("slider");
    expect(screen.getByRole("status")).toHaveTextContent(/^$/);

    // Half way down the rail (handle centre at the middle of its travel).
    fireEvent.pointerDown(handle, { pointerId: 1, clientY: TRACK.top + 22 + 293, button: 0 });
    const target = offsetOf(0.5, layout.height, window.innerHeight);
    expect(scrollTo).toHaveBeenLastCalledWith(0, expect.closeTo(target + 200, 0));
    const bubble = screen.getByRole("status");
    expect(bubble).toHaveTextContent("January 2024"); // half way through six years
    expect(Number(/([\d.]+)px/.exec(handle.style.transform)![1])).toBeCloseTo(293, -1); // the handle follows the finger

    // To the bottom: the oldest month; to the top: the newest.
    fireEvent.pointerMove(rail, { pointerId: 1, clientY: 5000 });
    expect(screen.getByRole("status")).toHaveTextContent("January 2021");
    fireEvent.pointerMove(rail, { pointerId: 1, clientY: -50 });
    expect(screen.getByRole("status")).toHaveTextContent("December 2026");
    expect(scrollTo).toHaveBeenLastCalledWith(0, 200);

    // Letting go hides the label; the page stays where it is.
    fireEvent.pointerUp(rail, { pointerId: 1, clientY: -50 });
    expect(screen.getByRole("status")).toHaveTextContent(/^$/);
  });

  it("jumps to exactly the top of a month when its year is tapped", () => {
    const layout = layoutOf(72);
    render(<Scrubber layout={layout} scrollMargin={200} />);
    fireEvent.focus(screen.getByRole("slider")); // awake
    const y2023 = Array.from(document.querySelectorAll<HTMLElement>("[data-year]")).find((e) => e.textContent === "2023")!;
    const mark = layout.months.find((m) => m.month === y2023.dataset.month)!;
    fireEvent.pointerDown(y2023, { pointerId: 2, clientY: 400, button: 0 });
    fireEvent.pointerUp(y2023, { pointerId: 2, clientY: 400 });
    expect(scrollTo).toHaveBeenLastCalledWith(0, mark.top + 200);
    expect(mark.label).toBe("December 2023");
  });

  it("jumps to the tapped point of the rail anywhere else", () => {
    const layout = layoutOf(72);
    render(<Scrubber layout={layout} scrollMargin={0} />);
    const rail = document.querySelector("[data-scrubber]")!;
    fireEvent.pointerDown(rail, { pointerId: 3, clientY: TRACK.top + 22, button: 0 });
    expect(scrollTo).not.toHaveBeenCalled(); // asleep: a touch at the edge of the page is not a jump
    fireEvent.pointerUp(rail, { pointerId: 3, clientY: TRACK.top + 22 });
    fireEvent.focus(screen.getByRole("slider"));
    fireEvent.pointerDown(rail, { pointerId: 3, clientY: TRACK.top + 22, button: 0 });
    fireEvent.pointerUp(rail, { pointerId: 3, clientY: TRACK.top + 22 });
    expect(scrollTo).toHaveBeenLastCalledWith(0, 0);
    fireEvent.pointerDown(rail, { pointerId: 4, clientY: TRACK.top + 22 + 586, button: 0 });
    fireEvent.pointerUp(rail, { pointerId: 4, clientY: TRACK.top + 22 + 586 });
    expect(scrollTo).toHaveBeenLastCalledWith(0, expect.closeTo(layout.height - window.innerHeight, 0));
  });

  it("follows the page's scroll position", async () => {
    const layout = layoutOf(72);
    render(<Scrubber layout={layout} scrollMargin={0} />);
    const handle = screen.getByRole("slider");
    Object.defineProperty(window, "scrollY", { value: (layout.height - window.innerHeight) / 2, configurable: true });
    await act(async () => {
      window.dispatchEvent(new Event("scroll"));
      await new Promise((r) => requestAnimationFrame(() => r(null)));
    });
    expect(handle.style.transform).toMatch(/translateY\(\d+(\.\d+)?px\)/);
    const y = Number(/translateY\(([\d.]+)px\)/.exec(handle.style.transform)![1]);
    expect(y).toBeGreaterThan(250);
    expect(y).toBeLessThan(350);
  });

  it("moves a month at a time with the arrow keys", () => {
    const layout = layoutOf(72);
    render(<Scrubber layout={layout} scrollMargin={0} />);
    const handle = screen.getByRole("slider");
    fireEvent.keyDown(handle, { key: "ArrowDown" });
    expect(scrollTo).toHaveBeenLastCalledWith(0, layout.months[1]!.top);
    fireEvent.keyDown(handle, { key: "End" });
    expect(scrollTo).toHaveBeenLastCalledWith(0, layout.height);
  });
});
