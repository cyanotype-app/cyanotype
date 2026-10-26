import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "../api";
import { AuthProvider } from "../auth";
import { UploadProvider } from "../upload/UploadProvider";
import { UploadPickerProvider } from "../shell/UploadPicker";
import "../test/dialogs";
import { detail, item, withRaw } from "../test/fixtures";
import { fakeLibrary } from "../test/fakeLibrary";
import { Library } from "./Library";

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderLibrary = (favorites = false) =>
  render(
    <MemoryRouter initialEntries={[favorites ? "/favorites" : "/photos"]}>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <UploadPickerProvider>
            <Library favorites={favorites} />
          </UploadPickerProvider>
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

/** Scroll the window the way a user would: the virtualizer listens for the scroll event. */
const scrollTo = (y: number) => {
  Object.defineProperty(window, "scrollY", { value: y, configurable: true });
  act(() => {
    window.dispatchEvent(new Event("scroll"));
  });
};

describe("Library", () => {
  it("offers browser uploads when empty", async () => {
    fakeLibrary([]);
    renderLibrary();
    expect(await screen.findByText(/drop photos or a folder/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Upload photos" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Upload folder" })).toBeInTheDocument();
  });

  it("queues picked photos, skips other files, and merges finished uploads into the grid", async () => {
    const lib = fakeLibrary([item(1)]);
    vi.spyOn(api, "checkUpload").mockResolvedValue({
      status: "upload",
      upload_id: "u",
      part_size: 16,
      parts: [{ number: 1, url: "/assets/uploads/u/parts/1" }],
      uploaded: [],
    });
    const put = vi.spyOn(api, "putPart").mockResolvedValue({ etag: '"e"' });
    const complete = vi.spyOn(api, "completeUpload").mockImplementation(async () => {
      lib.photos.push(item(2));
      return { asset_id: "id-2" };
    });
    const user = userEvent.setup({ applyAccept: false });
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");

    const input = screen.getByLabelText("Choose photos");
    await user.upload(input, [new File(["x"], "a.CR3"), new File(["x"], "notes.txt", { type: "text/plain" })]);
    expect(await screen.findByText(/1 file was skipped/)).toBeInTheDocument();
    await waitFor(() => expect(complete).toHaveBeenCalledWith("u", [{ number: 1, etag: '"e"' }]));
    expect(put).toHaveBeenCalledTimes(1);
    expect(await screen.findByAltText("photo-2.jpg")).toBeInTheDocument();
    expect(screen.getByAltText("photo-1.jpg")).toBeInTheDocument();
    expect(lib.timeline.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("groups photos under month and day headers, newest first", async () => {
    const photos = [
      item(1, { sort_time: "2026-10-14T09:00:00Z" }),
      item(2, { sort_time: "2026-10-14T07:00:00Z" }),
      item(3, { sort_time: "2026-10-02T07:00:00Z" }),
      item(4, { sort_time: "2025-12-31T23:59:59Z" }),
    ];
    fakeLibrary(photos);
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    expect(screen.getAllByRole("heading", { level: 2 }).map((h) => h.textContent)).toEqual(["October 2026", "December 2025"]);
    expect(screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["Wed, Oct 14", "Fri, Oct 2", "Wed, Dec 31"]);
    expect(screen.getAllByRole("img").map((i) => i.getAttribute("alt"))).toEqual(["photo-1.jpg", "photo-2.jpg", "photo-3.jpg", "photo-4.jpg"]);
    expect(screen.getByText("4 photos")).toBeInTheDocument();
  });

  it("only renders the tiles near the viewport and loads further pages as the page scrolls", async () => {
    // 600 photos, one every hour: 25 days.
    const many = Array.from({ length: 600 }, (_, n) =>
      item(n, { id: `id-${n}`, original_name: `photo-${n}.jpg`, sort_time: new Date(Date.UTC(2026, 5, 30, 23) - n * 3_600_000).toISOString() }),
    );
    const lib = fakeLibrary(many);
    renderLibrary();
    await screen.findByAltText("photo-0.jpg");
    const tiles = () => document.querySelectorAll("[data-tile]").length;
    expect(tiles()).toBeGreaterThan(5);
    expect(tiles()).toBeLessThan(80);
    expect(screen.queryByAltText("photo-300.jpg")).not.toBeInTheDocument();
    expect(lib.listAssets).toHaveBeenCalledTimes(1);

    // Far down the library (about the 300th photo): that page is fetched on its own, starting at its day.
    scrollTo(14_000);
    await waitFor(() => expect(lib.listAssets.mock.calls.length).toBeGreaterThan(1));
    const jumped = lib.listAssets.mock.calls.at(-1)![0] as { before?: string; cursor?: string };
    expect(jumped.before ?? jumped.cursor).toBeTruthy();
    await waitFor(() => expect(document.querySelectorAll("[data-tile] img").length).toBeGreaterThan(5));
    expect(screen.queryByAltText("photo-0.jpg")).not.toBeInTheDocument();
    expect(tiles()).toBeLessThan(100);
  });

  it("opens the lightbox with the keyboard and pages through the photos around it", async () => {
    fakeLibrary([item(1), item(2), item(3)]);
    const user = userEvent.setup();
    renderLibrary();
    expect(await screen.findByAltText("photo-1.jpg")).toHaveAttribute("src", "https://s3.test/thumb-1");

    // Keyboard: focus a tile, Enter opens, arrows navigate, Esc closes and focus returns to the tile.
    const tile = screen.getByRole("button", { name: "Open photo-2.jpg" });
    tile.focus();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();
    expect(screen.getByText("2 / 3")).toBeInTheDocument();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByAltText("photo-3.jpg").some((i) => i.getAttribute("src") === "https://s3.test/preview-3")).toBe(true));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(tile).toHaveFocus();
  });

  it("keeps paging in the lightbox past the end of what is loaded", async () => {
    // More photos than one page: the lightbox walks across the page boundary.
    const many = Array.from({ length: 130 }, (_, n) =>
      item(n, { id: `id-${n}`, original_name: `photo-${n}.jpg`, sort_time: new Date(Date.UTC(2026, 5, 30, 23) - n * 60_000).toISOString() }),
    );
    fakeLibrary(many);
    vi.mocked(api.getAsset).mockImplementation(async (id) => ({ ...detail(Number(id.split("-")[1])), id, original_name: `photo-${id.split("-")[1]}.jpg` }));
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Open photo-3.jpg" }));
    for (let i = 0; i < 125; i++) await user.keyboard("{ArrowRight}");
    expect(await screen.findByRole("dialog", { name: "photo-128.jpg" })).toBeInTheDocument();
    expect(screen.getByText("129 / 130")).toBeInTheDocument();
  }, 30_000);

  it("shows a placeholder for photos still being processed", async () => {
    fakeLibrary([item(1, { thumb_url: null })]);
    renderLibrary();
    expect(await screen.findByText("Processing…")).toBeInTheDocument();
  });

  describe("selecting and trashing", () => {
    const three = () => fakeLibrary([item(1), item(2), item(3), item(4)]);
    const tile = (n: number) => screen.getByRole("button", { name: new RegExp(`^(Open|Select) photo-${n}\\.jpg$`) });
    const present = (n: number) => screen.queryByAltText(`photo-${n}.jpg`) !== null;

    it("selects with clicks and shift-click ranges, select all and clear, then trashes with an Undo toast", async () => {
      three();
      const trash = vi.mocked(api.trashAssets);
      const restore = vi.mocked(api.restoreAssets);
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
      await waitFor(() => expect(screen.queryByRole("toolbar")).not.toBeInTheDocument()); // it slides out

      await user.click(screen.getByRole("button", { name: "Undo" }));
      expect(restore).toHaveBeenCalledWith(["id-2", "id-4"]);
      // Back in timeline order.
      await screen.findByAltText("photo-4.jpg");
      expect(screen.getAllByRole("img").map((i) => i.getAttribute("alt"))).toEqual(["photo-1.jpg", "photo-2.jpg", "photo-3.jpg", "photo-4.jpg"]);
      expect(await screen.findByText("2 restored")).toBeInTheDocument();
    });

    it("puts the photos back and says so when the server refuses", async () => {
      three();
      vi.mocked(api.trashAssets).mockRejectedValue(new ApiError(500, "internal error"));
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

    it("Select all covers photos that are not loaded: asks with the exact count, trashes the view and undoes the batch", async () => {
      const many = Array.from({ length: 250 }, (_, i) =>
        item(i + 1, { sort_time: `2026-03-${String(1 + Math.floor(i / 10)).padStart(2, "0")}T12:00:00Z` }),
      );
      const lib = fakeLibrary(many);
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-250.jpg");
      await user.click(screen.getByRole("button", { name: "Select" }));
      await user.click(screen.getByRole("button", { name: "Select all" }));
      expect(screen.getByText("250 selected")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Select all" })).toBeDisabled();
      // Deselect one loaded photo: the count follows.
      await user.click(tile(250));
      expect(screen.getByText("249 selected")).toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: "Move to trash" }));
      expect(screen.getByRole("heading", { name: "Move 249 photos to trash?" })).toBeInTheDocument();
      await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Move to trash" }));
      expect(await screen.findByText("249 moved to trash")).toBeInTheDocument();
      expect(api.trashAssets).toHaveBeenCalledWith({ scope: "library", except: ["id-250"] });
      expect(lib.trashed).toHaveLength(249);
      expect(lib.photos.map((p) => p.id)).toEqual(["id-250"]);

      await user.click(screen.getByRole("button", { name: "Undo" }));
      expect(await screen.findByText("249 restored")).toBeInTheDocument();
      expect(api.restoreAssets).toHaveBeenLastCalledWith({ scope: { trash_batch: "batch-1" }, except: [] });
      expect(lib.photos).toHaveLength(250);
    });

    it("Select all in Favorites unfavorites exactly the favorites view", async () => {
      const lib = fakeLibrary([item(1, { favorite: true }), item(2, { favorite: true }), item(3)]);
      const user = userEvent.setup();
      renderLibrary(true);
      await screen.findByAltText("photo-1.jpg");
      await user.click(screen.getByRole("button", { name: "Select" }));
      await user.click(screen.getByRole("button", { name: "Select all" }));
      await user.click(screen.getByRole("button", { name: "Unfavorite" }));
      expect(await screen.findByText("2 removed from favorites")).toBeInTheDocument();
      expect(api.favoriteAssets).toHaveBeenCalledWith({ scope: "favorites", except: [] }, false);
      expect(lib.photos.filter((p) => p.favorite === true)).toHaveLength(0);
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
      await waitFor(() => expect(screen.queryByRole("toolbar")).not.toBeInTheDocument()); // it slides out
      expect(tile(1)).toHaveAttribute("aria-label", "Open photo-1.jpg");
    });

    it("the Delete key asks first, then trashes the selection", async () => {
      three();
      const trash = vi.mocked(api.trashAssets);
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
      const trash = vi.mocked(api.trashAssets);
      const restore = vi.mocked(api.restoreAssets);
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

      // (the toast of the first trashing may still be sliding out)
      await user.click((await screen.findAllByRole("button", { name: "Undo" })).at(-1)!);
      expect(restore).toHaveBeenLastCalledWith(["id-4"]);
    });

    it("closes the lightbox when the last photo is trashed", async () => {
      fakeLibrary([item(1)]);
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
    const lib = fakeLibrary([withRaw(1), item(3)]);
    const unpair = vi.spyOn(api, "unpair").mockImplementation(async () => {
      lib.photos = lib.photos.map((p) => (p.id === "id-1" ? { ...p, paired: [] } : p));
      lib.photos.push(item(1, { id: "raw-1", original_name: "photo-1.CR3", sort_time: "2026-01-29T00:00:00Z" }));
    });
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
    fakeLibrary([withRaw(1)]);
    vi.spyOn(api, "unpair").mockRejectedValue(new ApiError(404, "not found"));
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
    await user.click(await screen.findByRole("button", { name: "Unpair" }));
    expect(await screen.findByText(/Couldn.t unpair/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Unpair" })).toBeInTheDocument();
  });

  describe("favorites", () => {
    const heart = (n: number, on: boolean) =>
      screen.getByRole("button", { name: `${on ? "Remove" : "Add"} photo-${n}.jpg ${on ? "from" : "to"} favorites` });

    it("hearts a tile at once, and takes it back with a message when the server refuses", async () => {
      fakeLibrary([item(1), item(2, { favorite: true })]);
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-1.jpg");
      expect(heart(2, true)).toHaveAttribute("aria-pressed", "true");
      expect(heart(1, false)).toHaveAttribute("aria-pressed", "false");

      await user.click(heart(1, false));
      expect(api.favoriteAssets).toHaveBeenCalledWith(["id-1"], true);
      expect(heart(1, true)).toBeInTheDocument();

      vi.mocked(api.favoriteAssets).mockRejectedValueOnce(new ApiError(500, "internal error"));
      await user.click(heart(2, true));
      expect(await screen.findByText(/Couldn’t remove from favorites\. internal error/)).toBeInTheDocument();
      expect(heart(2, true)).toBeInTheDocument(); // back to favorited
      expect(heart(1, true)).toBeInTheDocument(); // the other one is untouched
    });

    it("favorites and unfavorites the whole selection from the selection bar", async () => {
      fakeLibrary([item(1), item(2, { favorite: true }), item(3)]);
      const user = userEvent.setup();
      renderLibrary();
      await screen.findByAltText("photo-1.jpg");
      await user.click(screen.getByRole("button", { name: "Select" }));
      expect(screen.getByRole("button", { name: "Favorite" })).toBeDisabled();
      await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
      await user.click(screen.getByRole("button", { name: "Select photo-2.jpg" }));
      // A mix: the action is "Favorite" and hearts the ones that are not yet.
      await user.click(screen.getByRole("button", { name: "Favorite" }));
      expect(api.favoriteAssets).toHaveBeenCalledWith(["id-1", "id-2"], true);
      expect(await screen.findByText("2 added to favorites")).toBeInTheDocument();
      await waitFor(() => expect(screen.queryByRole("toolbar")).not.toBeInTheDocument()); // it slides out
      expect(heart(1, true)).toBeInTheDocument();

      // All favorites now: the same button un-hearts them.
      await user.click(screen.getByRole("button", { name: "Select" }));
      await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
      await user.click(screen.getByRole("button", { name: "Select photo-2.jpg" }));
      await user.click(screen.getByRole("button", { name: "Unfavorite" }));
      expect(api.favoriteAssets).toHaveBeenLastCalledWith(["id-1", "id-2"], false);
      expect(await screen.findByText("2 removed from favorites")).toBeInTheDocument();
      expect(heart(1, false)).toBeInTheDocument();
    });

    it("hearts from the lightbox", async () => {
      fakeLibrary([item(1), item(2)]);
      const user = userEvent.setup();
      renderLibrary();
      await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
      const box = screen.getByRole("dialog", { name: "photo-1.jpg" });
      await user.click(within(box).getByRole("button", { name: "Add to favorites" }));
      expect(api.favoriteAssets).toHaveBeenCalledWith(["id-1"], true);
      expect(within(box).getByRole("button", { name: "Remove from favorites" })).toHaveAttribute("aria-pressed", "true");
      await user.click(within(box).getByRole("button", { name: "Remove from favorites" }));
      expect(api.favoriteAssets).toHaveBeenLastCalledWith(["id-1"], false);
    });

    it("lists only favorites in the Favorites view, and unfavoriting a selection removes them from it", async () => {
      const lib = fakeLibrary([item(1), item(2, { favorite: true }), item(3), item(4, { favorite: true })]);
      const user = userEvent.setup();
      renderLibrary(true);
      await screen.findByAltText("photo-2.jpg");
      expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ favorite: true }));
      expect(lib.listAssets).toHaveBeenCalledWith(expect.objectContaining({ favorite: true }));
      expect(screen.getAllByRole("img").map((i) => i.getAttribute("alt"))).toEqual(["photo-2.jpg", "photo-4.jpg"]);
      expect(screen.getByText("2 photos")).toBeInTheDocument();

      await user.click(screen.getByRole("button", { name: "Select" }));
      await user.click(screen.getByRole("button", { name: "Select photo-2.jpg" }));
      await user.click(screen.getByRole("button", { name: "Unfavorite" }));
      await waitFor(() => expect(screen.queryByAltText("photo-2.jpg")).not.toBeInTheDocument());
      expect(screen.getByAltText("photo-4.jpg")).toBeInTheDocument();
      expect(screen.getByText("1 photo")).toBeInTheDocument();
      expect(await screen.findByText("1 removed from favorites")).toBeInTheDocument();
    });

    it("says how to add favorites when there are none", async () => {
      fakeLibrary([item(1)]);
      renderLibrary(true);
      expect(await screen.findByRole("heading", { name: "No favorites yet" })).toBeInTheDocument();
      expect(screen.getByRole("heading", { level: 1, name: "Favorites" })).toBeInTheDocument();
      expect(screen.getByRole("link", { name: "Browse photos" })).toHaveAttribute("href", "/photos");
    });
  });

  describe("downloads", () => {
    it("offers the original of a single photo as a plain link to the download endpoint", async () => {
      fakeLibrary([item(1)]);
      const user = userEvent.setup();
      renderLibrary();
      await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
      const link = within(screen.getByRole("dialog")).getByRole("link", { name: "Download" });
      expect(link).toHaveAttribute("href", "/assets/id-1/download");
    });
  });
});
