import { createRef } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ZoomControls } from "./ZoomControls";

function setup(zoomed = false) {
  const on = { in: vi.fn(), out: vi.fn(), fit: vi.fn() };
  const levelRef = createRef<HTMLSpanElement>();
  const announceRef = createRef<HTMLSpanElement>();
  const view = render(<ZoomControls zoomed={zoomed} onIn={on.in} onOut={on.out} onFit={on.fit} levelRef={levelRef} announceRef={announceRef} />);
  return { on, levelRef, announceRef, ...view };
}

describe("ZoomControls", () => {
  it("is a labelled group of three named buttons with their keys", () => {
    setup();
    const group = screen.getByRole("group", { name: "Zoom" });
    expect(group).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Zoom in" })).toHaveAttribute("aria-keyshortcuts", "+");
    expect(screen.getByRole("button", { name: "Zoom out" })).toHaveAttribute("aria-keyshortcuts", "-");
    expect(screen.getByRole("button", { name: "Fit photo to screen" })).toHaveAttribute("aria-keyshortcuts", "z");
  });

  it("runs the handlers and leaves focus on the pressed button", async () => {
    const { on } = setup();
    const user = userEvent.setup();
    const zin = screen.getByRole("button", { name: "Zoom in" });
    await user.click(zin);
    expect(on.in).toHaveBeenCalledTimes(1);
    expect(zin).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    await user.click(screen.getByRole("button", { name: "Fit photo to screen" }));
    expect(on.out).toHaveBeenCalledTimes(1);
    expect(on.fit).toHaveBeenCalledTimes(1);
  });

  it("starts at 100 %, hides the level from screen readers and offers a status for the settled value", () => {
    const { levelRef, announceRef } = setup();
    expect(levelRef.current).toHaveTextContent("100 %");
    expect(levelRef.current).toHaveAttribute("aria-hidden", "true");
    expect(announceRef.current).toHaveAttribute("aria-live", "polite");
  });

  it("marks itself on while zoomed (a touch screen shows it only then)", () => {
    const { rerender, on, levelRef, announceRef } = setup(false);
    // A touch screen hides it at fit (the class says so) and shows it once zoomed.
    expect(screen.getByRole("group", { name: "Zoom" }).className).toContain("pointer-coarse:hidden");
    rerender(<ZoomControls zoomed onIn={on.in} onOut={on.out} onFit={on.fit} levelRef={levelRef} announceRef={announceRef} />);
    expect(screen.getByRole("group", { name: "Zoom" }).className).not.toContain("pointer-coarse:hidden");
  });
});
