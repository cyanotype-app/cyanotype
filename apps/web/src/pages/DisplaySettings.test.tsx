import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type Prefs } from "../api";
import { DisplaySettings } from "./DisplaySettings";

afterEach(() => vi.restoreAllMocks());

const PREFS: Prefs = { memories_enabled: true, library_sort: "taken", stacks_collapsed: true, grid_style: "square" };

describe("DisplaySettings", () => {
  it("shows the saved layout and saves a new one", async () => {
    vi.spyOn(api, "prefs").mockResolvedValue(PREFS);
    const save = vi.spyOn(api, "setPrefs").mockResolvedValue({ ...PREFS, grid_style: "justified" });
    render(<DisplaySettings />);
    expect(await screen.findByRole("radio", { name: "Square" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("radio", { name: "Justified" }));
    await waitFor(() => expect(save).toHaveBeenCalledWith({ grid_style: "justified" }));
    expect(screen.getByRole("radio", { name: "Justified" })).toHaveAttribute("aria-checked", "true");
  });

  it("keeps the old layout when the server refuses the change", async () => {
    vi.spyOn(api, "prefs").mockResolvedValue(PREFS);
    vi.spyOn(api, "setPrefs").mockRejectedValue(new Error("no"));
    render(<DisplaySettings />);
    await userEvent.click(await screen.findByRole("radio", { name: "Justified" }));
    await waitFor(() => expect(screen.getByRole("radio", { name: "Square" })).toHaveAttribute("aria-checked", "true"));
  });
});
