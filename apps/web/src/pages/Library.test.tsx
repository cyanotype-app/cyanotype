import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "../api";
import { AuthProvider } from "../auth";
import { UploadProvider } from "../upload/UploadProvider";
import { detail, item, withRaw } from "../test/fixtures";
import { Library } from "./Library";

type IOCallback = (entries: { isIntersecting: boolean }[]) => void;
let observers: { cb: IOCallback; target?: Element }[] = [];

beforeEach(() => {
  observers = [];
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      entry: { cb: IOCallback; target?: Element };
      constructor(cb: IOCallback) {
        this.entry = { cb };
        observers.push(this.entry);
      }
      observe(el: Element) {
        this.entry.target = el;
      }
      disconnect() {}
      unobserve() {}
    },
  );
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderLibrary = () =>
  render(
    <MemoryRouter>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <Library />
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

describe("Library", () => {
  it("offers browser uploads when empty", async () => {
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [], next_before: null });
    renderLibrary();
    expect(await screen.findByText(/drop photos or a folder/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Upload photos" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Upload folder" })).toBeInTheDocument();
  });

  it("queues picked photos, skips other files, and merges finished uploads into the grid", async () => {
    const list = vi
      .spyOn(api, "listAssets")
      .mockResolvedValueOnce({ items: [item(1)], next_before: null })
      .mockResolvedValue({ items: [item(1), item(2)], next_before: null });
    vi.spyOn(api, "checkUpload").mockResolvedValue({
      status: "upload",
      upload_id: "u",
      part_size: 16,
      parts: [{ number: 1, url: "/assets/uploads/u/parts/1" }],
      uploaded: [],
    });
    const put = vi.spyOn(api, "putPart").mockResolvedValue({ etag: '"e"' });
    const complete = vi.spyOn(api, "completeUpload").mockResolvedValue({ asset_id: "id-2" });
    const user = userEvent.setup({ applyAccept: false });
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");

    const input = screen.getByLabelText("Choose photos");
    await user.upload(input, [new File(["x"], "a.CR3"), new File(["x"], "notes.txt", { type: "text/plain" })]);
    expect(await screen.findByText(/1 file was skipped/)).toBeInTheDocument();
    await waitFor(() => expect(complete).toHaveBeenCalledWith("u", [{ number: 1, etag: '"e"' }]));
    expect(put).toHaveBeenCalledTimes(1);
    expect(await screen.findByAltText("photo-2.jpg")).toBeInTheDocument();
    expect(list.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("renders a thumbnail grid, pages with `before` on scroll, and opens the lightbox with the keyboard", async () => {
    const list = vi
      .spyOn(api, "listAssets")
      .mockResolvedValueOnce({ items: [item(1), item(2)], next_cursor: "cursor-1", next_before: "2026-01-01T00:00:00Z" })
      .mockResolvedValueOnce({ items: [item(3)], next_cursor: null, next_before: null });
    const user = userEvent.setup();
    renderLibrary();

    expect(await screen.findByAltText("photo-1.jpg")).toHaveAttribute("src", "https://s3.test/thumb-1");
    expect(list).toHaveBeenCalledTimes(1);

    // The sentinel scrolls into view: the next page is requested with the cursor.
    await act(async () => observers.at(-1)!.cb([{ isIntersecting: true }]));
    await screen.findByAltText("photo-3.jpg");
    expect(list).toHaveBeenLastCalledWith({ cursor: "cursor-1", limit: 60 });

    // Keyboard: focus a tile, Enter opens, arrows navigate, Esc closes and focus returns to the tile.
    const tile = screen.getByRole("button", { name: "Open photo-2.jpg" });
    tile.focus();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByAltText("photo-3.jpg").some((i) => i.getAttribute("src") === "https://s3.test/preview-3")).toBe(true));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(tile).toHaveFocus();
  });

  it("falls back to `before` / `next_before` against an older server without cursors", async () => {
    const list = vi
      .spyOn(api, "listAssets")
      .mockResolvedValueOnce({ items: [item(1)], next_before: "2026-01-01T00:00:00Z" })
      .mockResolvedValueOnce({ items: [item(2)], next_before: null });
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    await act(async () => observers.at(-1)!.cb([{ isIntersecting: true }]));
    await screen.findByAltText("photo-2.jpg");
    expect(list).toHaveBeenLastCalledWith({ before: "2026-01-01T00:00:00Z", limit: 60 });
  });

  it("shows a placeholder for photos still being processed", async () => {
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [item(1, { thumb_url: null })], next_before: null });
    renderLibrary();
    expect(await screen.findByText("Processing…")).toBeInTheDocument();
  });

  describe("selecting and trashing", () => {
    const three = () => vi.spyOn(api, "listAssets").mockResolvedValue({ items: [item(1), item(2), item(3), item(4)], next_before: null });
    const tile = (n: number) => screen.getByRole("button", { name: new RegExp(`^(Open|Select) photo-${n}\\.jpg$`) });
    const present = (n: number) => screen.queryByAltText(`photo-${n}.jpg`) !== null;

    it("selects with clicks and shift-click ranges, select all and clear, then trashes with an Undo toast", async () => {
      three();
      const trash = vi.spyOn(api, "trashAssets").mockResolvedValue({ count: 2 });
      const restore = vi.spyOn(api, "restoreAssets").mockResolvedValue({ count: 2 });
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-1.jpg");

      await user.click(screen.getByRole("button", { name: "Select" }));
      expect(screen.getByText("Select photos")).toBeInTheDocument();
      await user.click(tile(1));
      expect(screen.getByText("1 selected")).toBeInTheDocument();
      expect(tile(1)).toHaveAttribute("aria-pressed", "true");
      await user.keyboard("{Shift>}");
      await user.click(tile(3));
      await user.keyboard("{/Shift}");
      expect(screen.getByText("3 selected")).toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: "Select all" }));
      expect(screen.getByText("4 selected")).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Clear" }));
      expect(screen.getByText("Select photos")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Move to trash" })).toBeDisabled();

      await user.click(tile(2));
      await user.click(tile(4));
      await user.click(screen.getByRole("button", { name: "Move to trash" }));

      // Optimistic: gone at once, no confirmation, selection mode ends.
      await waitFor(() => expect(present(2)).toBe(false));
      expect(present(4)).toBe(false);
      expect(present(1) && present(3)).toBe(true);
      expect(trash).toHaveBeenCalledWith(["id-2", "id-4"]);
      expect(await screen.findByText("2 moved to trash")).toBeInTheDocument();
      expect(screen.queryByRole("toolbar")).not.toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: "Undo" }));
      expect(restore).toHaveBeenCalledWith(["id-2", "id-4"]);
      // Back in timeline order.
      await screen.findByAltText("photo-4.jpg");
      expect(screen.getAllByRole("img").map((i) => i.getAttribute("alt"))).toEqual(["photo-1.jpg", "photo-2.jpg", "photo-3.jpg", "photo-4.jpg"]);
      expect(await screen.findByText("2 restored")).toBeInTheDocument();
    });

    it("puts the photos back and says so when the server refuses", async () => {
      three();
      vi.spyOn(api, "trashAssets").mockRejectedValue(new ApiError(500, "internal error"));
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-1.jpg");
      await user.click(screen.getByRole("button", { name: "Select" }));
      await user.click(tile(2));
      await user.click(screen.getByRole("button", { name: "Move to trash" }));
      expect(await screen.findByText(/Couldn’t move to trash\. internal error/)).toBeInTheDocument();
      expect(present(2)).toBe(true);
      expect(screen.getAllByRole("img").map((i) => i.getAttribute("alt"))).toEqual(["photo-1.jpg", "photo-2.jpg", "photo-3.jpg", "photo-4.jpg"]);
    });

    it("Escape clears the selection, then leaves selection mode", async () => {
      three();
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-1.jpg");
      await user.click(screen.getByRole("button", { name: "Select" }));
      await user.click(tile(1));
      await user.keyboard("{Escape}");
      expect(screen.getByText("Select photos")).toBeInTheDocument();
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("toolbar")).not.toBeInTheDocument();
      expect(tile(1)).toHaveAttribute("aria-label", "Open photo-1.jpg");
    });

    it("the Delete key asks first, then trashes the selection", async () => {
      three();
      const trash = vi.spyOn(api, "trashAssets").mockResolvedValue({ count: 1 });
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-1.jpg");
      await user.click(screen.getByRole("button", { name: "Select" }));
      await user.keyboard("{Delete}"); // nothing selected: nothing to ask
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
      await user.click(tile(3));
      await user.keyboard("{Delete}");
      expect(screen.getByRole("alertdialog", { name: "Move 1 photo to trash?" })).toBeInTheDocument();

      // Escape cancels the question without touching the selection.
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
      expect(screen.getByText("1 selected")).toBeInTheDocument();
      expect(trash).not.toHaveBeenCalled();

      await user.keyboard("{Delete}");
      await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Move to trash" }));
      await waitFor(() => expect(trash).toHaveBeenCalledWith(["id-3"]));
      expect(present(3)).toBe(false);
    });

    it("long-press on a touch device starts selecting; the press does not open the photo", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      try {
        three();
        renderLibrary();
        await screen.findByAltText("photo-1.jpg");
        const t = tile(2);
        fireEvent.pointerDown(t, { pointerType: "touch", clientX: 5, clientY: 5 });
        act(() => {
          vi.advanceTimersByTime(600);
        });
        fireEvent.pointerUp(t, { pointerType: "touch" });
        fireEvent.click(t);
        expect(screen.getByText("1 selected")).toBeInTheDocument();
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
        expect(tile(2)).toHaveAttribute("aria-pressed", "true");

        // Moving the finger first (a scroll) cancels it.
        fireEvent.pointerDown(tile(3), { pointerType: "touch", clientX: 5, clientY: 5 });
        fireEvent.pointerMove(tile(3), { pointerType: "touch", clientX: 5, clientY: 60 });
        act(() => {
          vi.advanceTimersByTime(600);
        });
        expect(screen.getByText("1 selected")).toBeInTheDocument();
      } finally {
        vi.useRealTimers();
      }
    });

    it("trashing from the lightbox moves on to the next photo, and Undo brings it back", async () => {
      three();
      const trash = vi.spyOn(api, "trashAssets").mockResolvedValue({ count: 1 });
      const restore = vi.spyOn(api, "restoreAssets").mockResolvedValue({ count: 1 });
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-1.jpg");
      await user.click(tile(2));
      expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: "Move to trash" }));
      expect(trash).toHaveBeenCalledWith(["id-2"]);
      expect(await screen.findByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();
      expect(await screen.findByText("1 moved to trash")).toBeInTheDocument();

      // The last photo: trashing it shows the previous one; the Delete key works inside the lightbox too.
      await user.keyboard("{ArrowRight}");
      expect(screen.getByRole("dialog", { name: "photo-4.jpg" })).toBeInTheDocument();
      await user.keyboard("{Delete}");
      expect(await screen.findByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: "Undo" }));
      expect(restore).toHaveBeenLastCalledWith(["id-4"]);
    });

    it("closes the lightbox when the last photo is trashed", async () => {
      vi.spyOn(api, "listAssets").mockResolvedValue({ items: [item(1)], next_before: null });
      vi.spyOn(api, "trashAssets").mockResolvedValue({ count: 1 });
      const user = userEvent.setup();
      renderLibrary();
      await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
      await user.click(screen.getByRole("button", { name: "Move to trash" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      expect(await screen.findByText("Your library is empty")).toBeInTheDocument();
      expect(screen.getByText("1 moved to trash")).toBeInTheDocument();
    });
  });

  it("unpairs from the lightbox: the badge goes and the RAW shows up as its own photo", async () => {
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [withRaw(1), item(3)], next_before: null });
    const unpair = vi.spyOn(api, "unpair").mockResolvedValue(undefined);
    vi.mocked(api.getAsset).mockImplementation(async (id) =>
      id === "raw-1"
        ? { ...detail(2), id: "raw-1", original_name: "photo-1.CR3", content_type: "image/x-canon-cr3" }
        : detail(Number(id.split("-")[1])),
    );
    const user = userEvent.setup();
    renderLibrary();
    const tile = await screen.findByRole("button", { name: "Open photo-1.jpg" });
    expect(tile).toHaveTextContent("RAW");

    await user.click(tile);
    await user.click(await screen.findByRole("button", { name: "Unpair" }));
    expect(unpair).toHaveBeenCalledWith("id-1");
    expect(await screen.findByText(/Unpaired/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(await screen.findByRole("button", { name: "Open photo-1.CR3" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open photo-1.jpg" })).not.toHaveTextContent("RAW");
  });

  it("keeps the pair when the server refuses to unpair", async () => {
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [withRaw(1)], next_before: null });
    vi.spyOn(api, "unpair").mockRejectedValue(new ApiError(404, "not found"));
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
    await user.click(await screen.findByRole("button", { name: "Unpair" }));
    expect(await screen.findByText(/Couldn.t unpair/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Unpair" })).toBeInTheDocument();
  });
});
