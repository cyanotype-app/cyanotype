import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "../api";
import { AuthProvider } from "../auth";
import { album, detail, item } from "../test/fixtures";
import { AlbumPage } from "./Album";
import { Albums } from "./Albums";

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
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderAt = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <Routes>
          <Route path="/albums" element={<Albums />} />
          <Route path="/albums/:id" element={<AlbumPage />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );

const tile = (n: number) => screen.getByRole("button", { name: new RegExp(`^(Open|Select) photo-${n}\\.jpg$`) });

describe("Albums page", () => {
  it("shows cards with count and date range, and creates an album", async () => {
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [album(3, { title: "Trip" }), album(0, { title: "Empty", cover: null, first_at: null, last_at: null })] });
    const create = vi.spyOn(api, "createAlbum").mockResolvedValue(album(7, { title: "Made" }));
    vi.spyOn(api, "getAlbum").mockResolvedValue(album(7, { title: "Made" }));
    vi.spyOn(api, "listAlbumAssets").mockResolvedValue({ items: [], next_cursor: null, next_before: null });
    const user = userEvent.setup();
    renderAt("/albums");

    const card = await screen.findByRole("link", { name: /Trip/ });
    expect(card).toHaveAttribute("href", "/albums/album-3");
    expect(card).toHaveTextContent("3 photos");
    expect(screen.getByRole("link", { name: /Empty/ })).toHaveTextContent("0 photos");

    await user.click(screen.getByRole("button", { name: "New album" }));
    await user.type(screen.getByLabelText("Album title"), "Made");
    await user.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ title: "Made" }));
    expect(await screen.findByRole("heading", { name: "Made" })).toBeInTheDocument();
  });

  it("invites you to make the first album", async () => {
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    renderAt("/albums");
    expect(await screen.findByText("No albums yet")).toBeInTheDocument();
  });
});

describe("Album page", () => {
  const setup = (photos = [item(1), item(2), item(3)]) => {
    vi.spyOn(api, "getAlbum").mockResolvedValue(album(3, { title: "Trip", description: "Sunny" }));
    const list = vi.spyOn(api, "listAlbumAssets").mockResolvedValue({ items: photos, next_cursor: null, next_before: null });
    renderAt("/albums/album-3");
    return list;
  };

  it("shows the title, description and photos", async () => {
    const list = setup();
    expect(await screen.findByRole("heading", { name: "Trip" })).toBeInTheDocument();
    expect(screen.getByText("Sunny")).toBeInTheDocument();
    expect(await screen.findByAltText("photo-2.jpg")).toBeInTheDocument();
    expect(list).toHaveBeenCalledWith("album-3", { cursor: null, limit: 60 });
  });

  it("shows a not-found message for an unknown album", async () => {
    vi.spyOn(api, "getAlbum").mockRejectedValue(new ApiError(404, "not found"));
    vi.spyOn(api, "listAlbumAssets").mockRejectedValue(new ApiError(404, "not found"));
    renderAt("/albums/nope");
    expect(await screen.findByRole("heading", { name: "Album not found" })).toBeInTheDocument();
  });

  it("edits the title and description", async () => {
    setup();
    const save = vi.spyOn(api, "updateAlbum").mockResolvedValue(album(3, { title: "Holiday", description: null }));
    const user = userEvent.setup();
    await screen.findByRole("heading", { name: "Trip" });
    await user.click(screen.getByRole("button", { name: "Edit" }));
    const title = screen.getByLabelText("Title");
    await user.clear(title);
    await user.type(title, "Holiday");
    await user.clear(screen.getByLabelText("Description"));
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalledWith("album-3", { title: "Holiday", description: null }));
    expect(await screen.findByRole("heading", { name: "Holiday" })).toBeInTheDocument();
  });

  it("removes the selected photos from the album", async () => {
    setup();
    const remove = vi.spyOn(api, "removeFromAlbum").mockResolvedValue({ count: 2 });
    const user = userEvent.setup();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    expect(screen.getByRole("button", { name: "Remove from album" })).toBeDisabled();
    await user.click(tile(1));
    await user.click(tile(3));
    await user.click(screen.getByRole("button", { name: "Remove from album" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("album-3", ["id-1", "id-3"]));
    await waitFor(() => expect(screen.queryByAltText("photo-1.jpg")).not.toBeInTheDocument());
    expect(screen.getByAltText("photo-2.jpg")).toBeInTheDocument();
    expect(screen.getByText("Removed 2 from the album")).toBeInTheDocument();
  });

  it("sets exactly one selected photo as the cover", async () => {
    setup();
    const patch = vi.spyOn(api, "updateAlbum").mockResolvedValue(album(3));
    const user = userEvent.setup();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(tile(1));
    await user.click(tile(2));
    expect(screen.getByRole("button", { name: "Set as cover" })).toBeDisabled();
    await user.click(tile(1));
    await user.click(screen.getByRole("button", { name: "Set as cover" }));
    await waitFor(() => expect(patch).toHaveBeenCalledWith("album-3", { cover_asset_id: "id-2" }));
    expect(await screen.findByText("Cover updated")).toBeInTheDocument();
  });

  it("deletes the album after confirming that the photos stay", async () => {
    setup();
    const del = vi.spyOn(api, "deleteAlbum").mockResolvedValue(undefined);
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    const user = userEvent.setup();
    await screen.findByRole("heading", { name: "Trip" });
    await user.click(screen.getByRole("button", { name: "Delete album" }));
    const dlg = screen.getByRole("alertdialog");
    expect(dlg).toHaveTextContent("The photos stay in your library");
    expect(del).not.toHaveBeenCalled();
    await user.click(within(dlg).getByRole("button", { name: "Delete album" }));
    await waitFor(() => expect(del).toHaveBeenCalledWith("album-3"));
    expect(await screen.findByRole("heading", { name: "Albums" })).toBeInTheDocument();
  });

  it("adds photos picked from the library, then reloads the album", async () => {
    const list = setup([item(1)]);
    const library = vi
      .spyOn(api, "listAssets")
      .mockResolvedValue({ items: [item(1), item(4), item(5)], next_cursor: null, next_before: null });
    const add = vi.spyOn(api, "addToAlbum").mockResolvedValue({ count: 2 });
    const user = userEvent.setup();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Add photos" }));

    const picker = await screen.findByRole("dialog", { name: "Add photos" });
    expect(library).toHaveBeenCalledWith({ cursor: null, limit: 60 });
    expect(within(picker).getByRole("button", { name: "Add" })).toBeDisabled();
    await user.click(await within(picker).findByRole("button", { name: "Select photo-4.jpg" }));
    await user.click(within(picker).getByRole("button", { name: "Select photo-5.jpg" }));
    await user.click(within(picker).getByRole("button", { name: "Add 2" }));

    await waitFor(() => expect(add).toHaveBeenCalledWith("album-3", ["id-4", "id-5"]));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add photos" })).not.toBeInTheDocument());
    expect(await screen.findByText("Added 2 to the album")).toBeInTheDocument();
    await waitFor(() => expect(list.mock.calls.length).toBeGreaterThan(1)); // list re-read from the top
  });

  it("hearts photos on the grid and in the lightbox, and takes the heart back when the server refuses", async () => {
    vi.spyOn(api, "getAlbum").mockResolvedValue(album(2));
    vi.spyOn(api, "listAlbumAssets").mockResolvedValue({ items: [item(1), item(2, { favorite: true })], next_cursor: null, next_before: null });
    const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 1 });
    const user = userEvent.setup();
    renderAt("/albums/album-2");
    await user.click(await screen.findByRole("button", { name: "Add photo-1.jpg to favorites" }));
    expect(fav).toHaveBeenCalledWith(["id-1"], true);
    expect(await screen.findByRole("button", { name: "Remove photo-1.jpg from favorites" })).toBeInTheDocument();

    fav.mockRejectedValueOnce(new ApiError(500, "nope"));
    await user.click(screen.getByRole("button", { name: "Remove photo-2.jpg from favorites" }));
    expect(await screen.findByText(/Couldn’t remove from favorites\. nope/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove photo-2.jpg from favorites" })).toBeInTheDocument();

    await user.click(tile(1));
    const box = screen.getByRole("dialog", { name: "photo-1.jpg" });
    await user.click(within(box).getByRole("button", { name: "Remove from favorites" }));
    expect(fav).toHaveBeenLastCalledWith(["id-1"], false);
  });
});
