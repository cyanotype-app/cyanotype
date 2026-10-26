import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { shareApi } from "../shareApi";
import { album, detail, item } from "../test/fixtures";
import { link } from "../test/shareFixtures";
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
  vi.spyOn(api, "getAlbum").mockResolvedValue(album(1, { title: "Trip", photo_count: 2 }));
  vi.spyOn(api, "listAlbumAssets").mockResolvedValue({ items: [item(1), item(2)], next_cursor: null, next_before: null });
  vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [album(1, { title: "Trip" })] });
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

describe("sharing from Albums", () => {
  it("shares the whole album from the album page", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1, { kind: "album" }));
    const user = userEvent.setup();
    renderAt("/albums/album-1");
    await user.click(await screen.findByRole("button", { name: "Share" }));
    expect(screen.getByRole("heading", { name: "Share the album Trip" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(create).toHaveBeenCalledWith(expect.objectContaining({ album_id: "album-1" }));
  });

  it("shares selected photos from the album's selection bar", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1));
    const user = userEvent.setup();
    renderAt("/albums/album-1");
    await user.click(await screen.findByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: /Select photo-2/ }));
    await user.click(within(screen.getByRole("toolbar", { name: "Selection" })).getByRole("button", { name: "Share" }));
    expect(screen.getByRole("heading", { name: "Share 1 photo" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(create).toHaveBeenLastCalledWith(expect.objectContaining({ asset_ids: ["id-2"] }));
  });
});
