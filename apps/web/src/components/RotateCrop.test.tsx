// The rotate/crop editor: keys, the handles (pointer and keyboard), the straighten slider, the ratios, saving and failing.
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { IDENTITY, type Recipe } from "../recipeText";
import { RotateCrop } from "./RotateCrop";

function setup(over: Partial<Parameters<typeof RotateCrop>[0]> = {}) {
  const onSave = vi.fn<(r: Recipe) => Promise<void>>(async () => {});
  const onRevert = vi.fn(async () => {});
  const onCancel = vi.fn();
  const utils = render(
    <RotateCrop name="beach.jpg" src="https://s3.test/base" initial={IDENTITY} fileSize={[4000, 3000]} onSave={onSave} onRevert={onRevert} onCancel={onCancel} {...over} />,
  );
  return { onSave, onRevert, onCancel, ...utils };
}

const image = () => document.querySelector<HTMLImageElement>('[data-testid="crop-frame"] img')!;
const box = () => screen.getByTestId("crop-box");
const pct = (el: HTMLElement, prop: "left" | "top" | "width" | "height") => parseFloat(el.style[prop]);

describe("the rotate and crop editor", () => {
  it("is a dialog with the picture, and Save waits for a change", () => {
    setup();
    expect(screen.getByRole("dialog", { name: "Rotate and crop" })).toBeInTheDocument();
    expect(image()).toHaveAttribute("src", "https://s3.test/base");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByText("Not edited")).toBeInTheDocument();
    // Not edited yet: nothing to take back.
    expect(screen.queryByRole("button", { name: "Revert to original" })).not.toBeInTheDocument();
  });

  it("rotates with r (Shift+R the other way), shows it, and saves with Enter", async () => {
    const user = userEvent.setup();
    const { onSave } = setup();
    await user.keyboard("r");
    expect(screen.getByText(/Rotated 90° right/)).toBeInTheDocument();
    expect(image().style.transform).toContain("rotate(90deg)");
    // The frame is turned: 3000 x 4000, so it is taller than wide.
    const wrap = image().closest<HTMLElement>("[data-testid='crop-frame']")!.parentElement!;
    expect(pct(wrap, "height")).toBeGreaterThan(pct(wrap, "width"));
    await user.keyboard("R");
    expect(screen.getByText("Not edited")).toBeInTheDocument();
    await user.keyboard("{Shift>}r{/Shift}");
    expect(screen.getByText(/Rotated 90° left/)).toBeInTheDocument();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(onSave.mock.calls[0]![0]).toEqual({ rotate: 270, straighten: 0, crop: null });
  });

  it("rotates with the buttons, and Escape cancels without saving", async () => {
    const user = userEvent.setup();
    const { onSave, onCancel } = setup();
    await user.click(screen.getByRole("button", { name: "Rotate right" }));
    await user.click(screen.getByRole("button", { name: "Rotate right" }));
    expect(screen.getByText(/Rotated 180°/)).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onSave).not.toHaveBeenCalled();
  });

  it("opens with the first turn already made when r opened it", () => {
    setup({ startTurned: true });
    expect(screen.getByText(/Rotated 90° right/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("straightens with the slider, magnifying the picture so no corner shows", () => {
    const { onSave } = setup();
    fireEvent.change(screen.getByRole("slider"), { target: { value: "2.5" } });
    expect(screen.getByText(/Straightened 2.5° right/)).toBeInTheDocument();
    expect(screen.getByText("2.5°")).toBeInTheDocument();
    expect(image().style.transform).toContain("rotate(2.5deg)");
    const scale = Number(/scale\(([\d.]+)\)/.exec(image().style.transform)![1]);
    expect(scale).toBeGreaterThan(1);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSave).toHaveBeenCalledWith({ rotate: 0, straighten: 2.5, crop: null });
  });

  it("shows eight handles and the crop area with c, and c again hides them but keeps the crop", async () => {
    const user = userEvent.setup();
    setup();
    expect(screen.queryByTestId("crop-box")).not.toBeInTheDocument();
    await user.keyboard("c");
    expect(screen.getAllByRole("button", { name: /^Crop (corner|edge)/ })).toHaveLength(8);
    expect(screen.getByRole("group", { name: /Crop area/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Crop" })).toHaveAttribute("aria-pressed", "true");
    await user.keyboard("c");
    expect(screen.queryByRole("button", { name: /^Crop (corner|edge)/ })).not.toBeInTheDocument();
    expect(box()).toBeInTheDocument();
  });

  it("drags a corner with a finger or a mouse, and the crop follows", () => {
    const { onSave } = setup({ startCropping: true });
    const before = { w: pct(box(), "width"), h: pct(box(), "height") };
    const se = screen.getByRole("button", { name: "Crop corner, bottom right" });
    // The frame is 320 x 240 here; the crop starts at 5% in, 90% across.
    fireEvent.pointerDown(se, { pointerId: 1, pointerType: "touch", clientX: 300, clientY: 220, button: 0 });
    fireEvent.pointerMove(se, { pointerId: 1, pointerType: "touch", clientX: 268, clientY: 196 });
    fireEvent.pointerUp(se, { pointerId: 1, pointerType: "touch" });
    // 32 px of 320 is 10% across; 24 px of 240 is 10% down; the corner was dragged inward.
    expect(pct(box(), "width")).toBeCloseTo(before.w - 32, 0);
    expect(pct(box(), "height")).toBeCloseTo(before.h - 24, 0);
    // After the finger lifts, moving does nothing.
    fireEvent.pointerMove(se, { pointerId: 1, clientX: 100, clientY: 100 });
    expect(pct(box(), "width")).toBeCloseTo(before.w - 32, 0);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const saved = onSave.mock.calls[0]![0];
    expect(saved.crop!.x).toBeCloseTo(0.05, 3);
    expect(saved.crop!.w).toBeCloseTo(0.8, 3);
    expect(saved.crop!.h).toBeCloseTo(0.8, 3);
  });

  it("moves the whole crop by its middle, never out of the picture", () => {
    setup({ startCropping: true });
    const area = box();
    fireEvent.pointerDown(area, { pointerId: 2, pointerType: "mouse", clientX: 100, clientY: 100, button: 0 });
    fireEvent.pointerMove(area, { pointerId: 2, clientX: 900, clientY: 900 });
    fireEvent.pointerUp(area, { pointerId: 2 });
    expect(pct(area, "left") + pct(area, "width")).toBeCloseTo(320, 0);
    expect(pct(area, "top") + pct(area, "height")).toBeCloseTo(240, 0);
  });

  it("resizes from the keyboard: arrows on a handle, Shift for bigger steps", async () => {
    const user = userEvent.setup();
    setup({ startCropping: true });
    const before = pct(box(), "width");
    const e = screen.getByRole("button", { name: "Crop edge, right" });
    e.focus();
    await user.keyboard("{ArrowLeft}");
    expect(pct(box(), "width")).toBeCloseTo(before - 3.2, 1);
    await user.keyboard("{Shift>}{ArrowLeft}{/Shift}");
    expect(pct(box(), "width")).toBeCloseTo(before - 3.2 - 16, 1);
    // The area moves with arrows too.
    const left = pct(box(), "left");
    box().focus();
    await user.keyboard("{ArrowRight}");
    expect(pct(box(), "left")).toBeCloseTo(left + 3.2, 1);
  });

  it("fits a ratio into the crop, in pixels, and turns it with the picture", async () => {
    const user = userEvent.setup();
    const { onSave } = setup({ startCropping: true });
    await user.click(screen.getByRole("button", { name: "1:1" }));
    const pixelsWide = pct(box(), "width") * (4000 / 320);
    const pixelsHigh = pct(box(), "height") * (3000 / 240);
    expect(pixelsWide).toBeCloseTo(pixelsHigh, 0);
    // A 3:2 crop becomes 2:3 when the picture is turned.
    await user.click(screen.getByRole("button", { name: "3:2" }));
    expect(screen.getByRole("button", { name: "3:2" })).toHaveAttribute("aria-pressed", "true");
    await user.keyboard("r");
    expect(screen.getByRole("button", { name: "2:3" })).toHaveAttribute("aria-pressed", "true");
    // A drag keeps the ratio.
    const se = screen.getByRole("button", { name: "Crop corner, bottom right" });
    fireEvent.pointerDown(se, { pointerId: 3, clientX: 10, clientY: 10, button: 0 });
    fireEvent.pointerMove(se, { pointerId: 3, clientX: -10, clientY: 0 });
    fireEvent.pointerUp(se, { pointerId: 3 });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const c = onSave.mock.calls[0]![0].crop!;
    // The frame is 3000 x 4000 now.
    expect((c.w * 3000) / (c.h * 4000)).toBeCloseTo(2 / 3, 3);
  });

  it("resets to the saved recipe, and offers to go back to the original only when there is one", async () => {
    const user = userEvent.setup();
    const saved: Recipe = { rotate: 90, straighten: 0, crop: { x: 0.1, y: 0.1, w: 0.5, h: 0.5 } };
    const { onRevert } = setup({ initial: saved });
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByText(/Rotated 90° right, cropped to 1500 × 2000/)).toBeInTheDocument();
    await user.keyboard("r");
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Reset" }));
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByText(/Rotated 90° right/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Revert to original" }));
    expect(onRevert).toHaveBeenCalledTimes(1);
  });

  it("saves a recipe that changes nothing as no change, and says why it failed when saving fails", async () => {
    const user = userEvent.setup();
    const onSave = vi.fn().mockRejectedValueOnce(new Error("Could not save the changes. Try again.")).mockResolvedValue(undefined);
    setup({ onSave });
    await user.keyboard("r");
    await user.keyboard("R");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.keyboard("r");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save the changes. Try again.");
    // Still open, and it can be tried again.
    expect(screen.getByRole("dialog", { name: "Rotate and crop" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("does not take Enter from a button that has the focus", async () => {
    const user = userEvent.setup();
    const { onSave, onCancel } = setup();
    await user.keyboard("r");
    screen.getByRole("button", { name: "Cancel" }).focus();
    await user.keyboard("{Enter}");
    expect(onSave).not.toHaveBeenCalled();
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("keeps Tab inside the dialog", async () => {
    const user = userEvent.setup();
    setup();
    const first = screen.getByRole("button", { name: "Cancel" });
    first.focus();
    await user.tab({ shift: true });
    expect(screen.getByRole("dialog").contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(first);
  });
});
