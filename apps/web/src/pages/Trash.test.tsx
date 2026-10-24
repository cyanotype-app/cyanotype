import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type TrashList } from "../api";
import { AuthProvider } from "../auth";
import { trashed } from "../test/fixtures";
import { Trash } from "./Trash";

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

const list = (n: number, over: Partial<TrashList> = {}): TrashList => ({
  items: Array.from({ length: n }, (_, i) => trashed(i + 1)),
  next_cursor: null,
  total: n,
  retention_days: 30,
  ...over,
});

const renderTrash = () =>
  render(
    <MemoryRouter>
      <AuthProvider>
        <Trash />
      </AuthProvider>
    </MemoryRouter>,
  );
const tile = (n: number) => screen.getByRole("button", { name: `Select photo-${n}.jpg` });
const present = (n: number) => screen.queryByAltText(`photo-${n}.jpg`) !== null;

describe("Trash page", () => {
  it("shows trashed photos with how long they have left", async () => {
    vi.spyOn(api, "listTrash").mockResolvedValue(list(2));
    renderTrash();
    expect(await screen.findByAltText("photo-1.jpg")).toBeInTheDocument();
    expect(screen.getAllByText("Deleted 3 days ago · gone in 28 days")).toHaveLength(2);
    expect(screen.getByText(/deleted forever 30 days after/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Empty trash" })).toBeInTheDocument();
  });

  it("says so when the trash is empty", async () => {
    vi.spyOn(api, "listTrash").mockResolvedValue(list(0));
    renderTrash();
    expect(await screen.findByText("The trash is empty.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Empty trash" })).not.toBeInTheDocument();
  });

  it("restores the selected photos (click and shift-click range)", async () => {
    vi.spyOn(api, "listTrash").mockResolvedValue(list(4));
    const restore = vi.spyOn(api, "restoreAssets").mockResolvedValue({ count: 3 });
    const user = userEvent.setup();
    renderTrash();
    await screen.findByAltText("photo-1.jpg");
    expect(screen.getByRole("button", { name: "Restore" })).toBeDisabled();

    await user.click(tile(1));
    await user.keyboard("{Shift>}");
    await user.click(tile(3));
    await user.keyboard("{/Shift}");
    expect(screen.getByText("3 selected")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Restore" }));

    expect(restore).toHaveBeenCalledWith(["id-1", "id-2", "id-3"]);
    await waitFor(() => expect(present(1)).toBe(false));
    expect(present(4)).toBe(true);
    expect(await screen.findByText("3 restored to your library")).toBeInTheDocument();
  });

  it("asks before deleting forever, and says it cannot be undone", async () => {
    vi.spyOn(api, "listTrash").mockResolvedValue(list(3));
    const purge = vi.spyOn(api, "purgeAssets").mockResolvedValue({ count: 2 });
    const user = userEvent.setup();
    renderTrash();
    await screen.findByAltText("photo-1.jpg");
    await user.click(tile(2));
    await user.click(tile(3));
    await user.click(screen.getByRole("button", { name: "Delete forever" }));

    const dialog = screen.getByRole("alertdialog", { name: "Delete 2 photos forever?" });
    expect(within(dialog).getByText("This can’t be undone.")).toBeInTheDocument();
    // Safe default: Cancel has focus, and cancelling deletes nothing.
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(purge).not.toHaveBeenCalled();
    expect(screen.getByText("2 selected")).toBeInTheDocument(); // Escape closed the dialog only

    await user.click(screen.getByRole("button", { name: "Delete forever" }));
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete forever" }));
    expect(purge).toHaveBeenCalledWith(["id-2", "id-3"]);
    await waitFor(() => expect(present(2)).toBe(false));
    expect(present(1)).toBe(true);
    expect(await screen.findByText("2 deleted forever")).toBeInTheDocument();
  });

  it("Select all means the whole trash: the exact count, one scope request, then the list starts over", async () => {
    const lt = vi.spyOn(api, "listTrash").mockResolvedValue(list(4, { total: 500, next_cursor: "more" }));
    const purge = vi.spyOn(api, "purgeAssets").mockResolvedValue({ count: 499 });
    const restore = vi.spyOn(api, "restoreAssets").mockResolvedValue({ count: 499 });
    const user = userEvent.setup();
    renderTrash();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select all" }));
    expect(screen.getByText("500 selected")).toBeInTheDocument();
    await user.click(tile(1)); // deselect one
    expect(screen.getByText("499 selected")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Delete forever" }));
    const dialog = screen.getByRole("alertdialog", { name: "Delete 499 photos forever?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete forever" }));
    expect(purge).toHaveBeenCalledWith({ scope: "trash", except: ["id-1"] });
    expect(await screen.findByText("499 deleted forever")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: "Restore" }));
    await waitFor(() => expect(restore).toHaveBeenCalledWith({ scope: "trash", except: [] }));
    expect(lt.mock.calls.length).toBeGreaterThanOrEqual(3); // first page, then re-read after each whole-trash action
  });

  it("empties the trash after confirming with the total count (more than what is loaded)", async () => {
    vi.spyOn(api, "listTrash").mockResolvedValue(list(2, { total: 140, next_cursor: "more" }));
    const empty = vi.spyOn(api, "emptyTrash").mockResolvedValue({ count: 140 });
    const user = userEvent.setup();
    renderTrash();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Empty trash" }));
    const dialog = screen.getByRole("alertdialog", { name: "Empty the trash?" });
    expect(within(dialog).getByText(/All 140 photos in the trash will be permanently deleted/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete 140 photos forever" }));
    expect(empty).toHaveBeenCalledOnce();
    expect(await screen.findByText("The trash is empty.")).toBeInTheDocument();
    expect(screen.getByText("Trash emptied · 140 deleted forever")).toBeInTheDocument();
  });

  it("keeps the photos and reports the error when a restore fails", async () => {
    vi.spyOn(api, "listTrash").mockResolvedValue(list(2));
    vi.spyOn(api, "restoreAssets").mockRejectedValue(new ApiError(502, "storage unavailable"));
    const user = userEvent.setup();
    renderTrash();
    await screen.findByAltText("photo-1.jpg");
    await user.click(tile(1));
    await user.click(screen.getByRole("button", { name: "Restore" }));
    expect(await screen.findByText(/Couldn’t restore\. storage unavailable/)).toBeInTheDocument();
    expect(present(1)).toBe(true);
  });

  it("loads the next page with the cursor", async () => {
    let io: ((e: { isIntersecting: boolean }[]) => void) | undefined;
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        constructor(cb: typeof io) {
          io = cb;
        }
        observe() {}
        disconnect() {}
      },
    );
    const lt = vi
      .spyOn(api, "listTrash")
      .mockResolvedValueOnce(list(2, { next_cursor: "c1", total: 3 }))
      .mockResolvedValueOnce({ ...list(0), items: [trashed(3)], total: 3 });
    renderTrash();
    await screen.findByAltText("photo-1.jpg");
    // The first page renders before the observer is attached (a render later), so wait for it.
    await waitFor(() => expect(io).toBeDefined());
    io!([{ isIntersecting: true }]);
    await screen.findByAltText("photo-3.jpg");
    expect(lt).toHaveBeenLastCalledWith({ cursor: "c1", limit: 60 });
  });
});
