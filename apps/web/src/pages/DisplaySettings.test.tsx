import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type Prefs } from "../api";
import { DisplaySettings } from "./DisplaySettings";

afterEach(() => vi.restoreAllMocks());

const PREFS: Prefs = { memories_enabled: true, library_sort: "taken", stacks_collapsed: true };

describe("DisplaySettings", () => {
  it("shows the saved order and saves a new one", async () => {
    vi.spyOn(api, "prefs").mockResolvedValue(PREFS);
    const save = vi.spyOn(api, "setPrefs").mockResolvedValue({ ...PREFS, library_sort: "added" });
    render(<DisplaySettings />);
    expect(await screen.findByRole("radio", { name: "Taken" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("radio", { name: "Added" }));
    await waitFor(() => expect(save).toHaveBeenCalledWith({ library_sort: "added" }));
    expect(screen.getByRole("radio", { name: "Added" })).toHaveAttribute("aria-checked", "true");
  });

  it("keeps the old order when the server refuses the change", async () => {
    vi.spyOn(api, "prefs").mockResolvedValue(PREFS);
    vi.spyOn(api, "setPrefs").mockRejectedValue(new Error("no"));
    render(<DisplaySettings />);
    await userEvent.click(await screen.findByRole("radio", { name: "Added" }));
    await waitFor(() => expect(screen.getByRole("radio", { name: "Taken" })).toHaveAttribute("aria-checked", "true"));
  });
});
