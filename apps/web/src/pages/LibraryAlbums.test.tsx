import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { UploadProvider } from "../upload/UploadProvider";
import { UploadPickerProvider } from "../shell/UploadPicker";
import "../test/dialogs";
import { album, detail, item } from "../test/fixtures";
import { fakeLibrary } from "../test/fakeLibrary";
import { Library } from "./Library";

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  fakeLibrary([item(1), item(2), item(3)]);
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => ({
    ...detail(Number(id.split("-")[1])),
    albums: id === "id-1" ? [{ id: "album-9", title: "Summer" }] : [],
  }));
});
afterEach(() => {
  vi.restoreAllMocks();
});

const renderLibrary = () =>
  render(
    <MemoryRouter>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <UploadPickerProvider>
            <Library />
          </UploadPickerProvider>
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

describe("Library: add to album", () => {
  it("adds the selection to a chosen album, searching by title, and toasts the result", async () => {
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [album(1, { title: "Summer" }), album(2, { title: "Winter" })] });
    const add = vi.spyOn(api, "addToAlbum").mockResolvedValue({ count: 2 });
    const user = userEvent.setup();
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");

    await user.click(screen.getByRole("button", { name: "Select" }));
    expect(screen.getByRole("button", { name: "Add to album" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    await user.click(screen.getByRole("button", { name: "Select photo-3.jpg" }));
    await user.click(screen.getByRole("button", { name: "Add to album" }));

    const dialog = await screen.findByRole("dialog", { name: "Add 2 photos to album" });
    expect(await within(dialog).findByText("Winter")).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Search albums"), "sum");
    expect(within(dialog).queryByText("Winter")).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /Summer/ }));

    await waitFor(() => expect(add).toHaveBeenCalledWith("album-1", ["id-1", "id-3"]));
    expect(await screen.findByText("Added 2 to Summer")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("creates a new album inline with the selected photos", async () => {
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    const create = vi.spyOn(api, "createAlbum").mockResolvedValue(album(5, { title: "Fresh", photo_count: 1 }));
    const user = userEvent.setup();
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-2.jpg" }));
    await user.click(screen.getByRole("button", { name: "Add to album" }));

    expect(await screen.findByText("No albums yet.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "New album" }));
    await user.type(screen.getByLabelText("Album title"), "Fresh");
    await user.click(screen.getByRole("button", { name: "Create and add" }));

    await waitFor(() => expect(create).toHaveBeenCalledWith({ title: "Fresh", asset_ids: ["id-2"] }));
    expect(await screen.findByText("Created Fresh with 1 photo")).toBeInTheDocument();
  });

  it("says so when the photos were already in the album, and shows errors without closing", async () => {
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [album(1, { title: "Summer" })] });
    const add = vi
      .spyOn(api, "addToAlbum")
      .mockRejectedValueOnce(new Error("boom"))
      .mockResolvedValueOnce({ count: 0 });
    const user = userEvent.setup();
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    await user.click(screen.getByRole("button", { name: "Add to album" }));
    await user.click(await screen.findByRole("button", { name: /Summer/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
    await user.click(screen.getByRole("button", { name: /Summer/ }));
    expect(await screen.findByText("That photo is already in Summer")).toBeInTheDocument();
    expect(add).toHaveBeenCalledTimes(2);
  });

  it("adds from the lightbox, which also lists the photo's albums", async () => {
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [album(1, { title: "Winter" })] });
    const add = vi.spyOn(api, "addToAlbum").mockResolvedValue({ count: 1 });
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
    const box = screen.getByRole("dialog", { name: "photo-1.jpg" });
    expect(await within(box).findByRole("link", { name: "Summer" })).toHaveAttribute("href", "/albums/album-9");

    await user.click(within(box).getByRole("button", { name: "Add to album" }));
    await user.click(await screen.findByRole("button", { name: /Winter/ }));
    await waitFor(() => expect(add).toHaveBeenCalledWith("album-1", ["id-1"]));
    expect(await screen.findByText("Added 1 to Winter")).toBeInTheDocument();
    // The lightbox is still open and re-read the photo's details.
    expect(screen.getByRole("dialog", { name: "photo-1.jpg" })).toBeInTheDocument();
    await waitFor(() => expect(vi.mocked(api.getAsset).mock.calls.filter(([id]) => id === "id-1").length).toBeGreaterThan(1));
  });

  it("Escape closes only the dialog, not the lightbox under it", async () => {
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
    await user.click(screen.getByRole("button", { name: "Add to album" }));
    await screen.findByRole("dialog", { name: "Add 1 photo to album" });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Add 1 photo to album" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "photo-1.jpg" })).toBeInTheDocument();
  });
});
