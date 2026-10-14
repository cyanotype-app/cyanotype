import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { UploadProvider } from "../upload/UploadProvider";
import { detail, item } from "../test/fixtures";
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
});
