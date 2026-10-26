import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmSheet } from "./ConfirmSheet";
import { Modal } from "./sheetForm";

afterEach(() => vi.restoreAllMocks());

describe("Modal", () => {
  it("closes on Escape before anything behind it hears the key, and gives focus back", async () => {
    const behind = vi.fn();
    document.addEventListener("keydown", behind);
    function Page() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button onClick={() => setOpen(true)}>Open it</button>
          {open && (
            <Modal title="Hello" onClose={() => setOpen(false)}>
              <input aria-label="Name" data-autofocus="" />
            </Modal>
          )}
        </>
      );
    }
    const user = userEvent.setup();
    render(<Page />);
    const opener = screen.getByRole("button", { name: "Open it" });
    await user.click(opener);
    expect(await screen.findByRole("dialog", { name: "Hello" })).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(behind.mock.calls.some(([e]) => (e as KeyboardEvent).key === "Escape")).toBe(false);
    expect(opener).toHaveFocus();
    document.removeEventListener("keydown", behind);
  });
});

describe("ConfirmSheet", () => {
  it("puts focus on Cancel for a destructive question, so Enter never destroys", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <ConfirmSheet title="Delete it?" confirmLabel="Delete" danger onConfirm={onConfirm} onCancel={onCancel}>
        <p>Gone for good.</p>
      </ConfirmSheet>,
    );
    expect(await screen.findByRole("dialog", { name: "Delete it?" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onCancel).toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("puts focus on the answer for a harmless one", async () => {
    render(
      <ConfirmSheet title="Move to trash?" confirmLabel="Move" onConfirm={() => {}} onCancel={() => {}}>
        <p>You can restore them.</p>
      </ConfirmSheet>,
    );
    await screen.findByRole("dialog");
    expect(screen.getByRole("button", { name: "Move" })).toHaveFocus();
  });
});
