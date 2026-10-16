import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { shareApi } from "../shareApi";
import { pubInfo, pubItem } from "../test/shareFixtures";
import { ShareView } from "./ShareView";

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const renderView = (token = "tok") =>
  render(
    <MemoryRouter initialEntries={[`/s/${token}`]}>
      <Routes>
        <Route path="/s/:token" element={<ShareView />} />
      </Routes>
    </MemoryRouter>,
  );

const open = (items = [pubItem(1), pubItem(2), pubItem(3)], info = pubInfo()) => {
  vi.spyOn(shareApi, "publicInfo").mockResolvedValue(info);
  vi.spyOn(shareApi, "publicItems").mockResolvedValue({ items, next_cursor: null });
};

describe("ShareView", () => {
  it("shows the title, the count and a grid of thumbnails without any file names", async () => {
    open();
    renderView();
    expect(await screen.findByRole("heading", { name: "Summer trip" })).toBeInTheDocument();
    expect(screen.getByText("3 photos")).toBeInTheDocument();
    const grid = await screen.findByRole("list", { name: "Photos" });
    const tiles = within(grid).getAllByRole("button");
    expect(tiles).toHaveLength(3);
    expect(tiles[0]).toHaveAccessibleName("Open photo 1 of 3");
    expect(grid.querySelector("img")).toHaveAttribute("src", "https://s3.test/thumb-1");
    expect(document.body.textContent).not.toMatch(/\.jpg|IMG_/i);
    // No signed-in chrome: no links to the library, no sign-in.
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });

  it("asks search engines and referrers to stay away, and cleans up after itself", async () => {
    open();
    const { unmount } = renderView();
    await screen.findByRole("heading", { name: "Summer trip" });
    expect(document.querySelector('meta[name="robots"]')).toHaveAttribute("content", expect.stringContaining("noindex"));
    expect(document.querySelector('meta[name="referrer"]')).toHaveAttribute("content", "no-referrer");
    expect(document.title).toBe("Summer trip · Cyanotype");
    unmount();
    expect(document.querySelector('meta[name="robots"]')).toBeNull();
  });

  it("says the same thing for an unknown, revoked or expired link", async () => {
    vi.spyOn(shareApi, "publicInfo").mockRejectedValue(new ApiError(404, "not found"));
    renderView();
    expect(await screen.findByRole("heading", { name: "This link is not available" })).toBeInTheDocument();
    expect(screen.getByText(/expired or been turned off/)).toBeInTheDocument();
  });

  it("reports other failures with a retry", async () => {
    const info = vi
      .spyOn(shareApi, "publicInfo")
      .mockRejectedValueOnce(new ApiError(0, "Cannot reach the server."))
      .mockResolvedValue(pubInfo());
    vi.spyOn(shareApi, "publicItems").mockResolvedValue({ items: [pubItem(1)], next_cursor: null });
    const user = userEvent.setup();
    renderView();
    expect(await screen.findByRole("alert")).toHaveTextContent("Cannot reach the server.");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { name: "Summer trip" })).toBeInTheDocument();
    expect(info).toHaveBeenCalledTimes(2);
  });

  describe("password", () => {
    it("shows only a password form until it is right, then the photos", async () => {
      vi.spyOn(shareApi, "publicInfo")
        .mockResolvedValueOnce({ needs_password: true, unlocked: false })
        .mockResolvedValue(pubInfo({ needs_password: true }));
      const items = vi.spyOn(shareApi, "publicItems").mockResolvedValue({ items: [pubItem(1)], next_cursor: null });
      const unlock = vi
        .spyOn(shareApi, "publicUnlock")
        .mockRejectedValueOnce(new ApiError(401, "unauthorized"))
        .mockResolvedValue(undefined);
      const user = userEvent.setup();
      renderView("secret-token");
      expect(await screen.findByRole("heading", { name: "This link is protected" })).toBeInTheDocument();
      expect(screen.queryByRole("list", { name: "Photos" })).not.toBeInTheDocument();
      expect(items).not.toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "View photos" })).toBeDisabled();

      await user.type(screen.getByLabelText("Password"), "wrong");
      await user.click(screen.getByRole("button", { name: "View photos" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("That password is not right.");
      expect(unlock).toHaveBeenCalledWith("secret-token", "wrong");

      await user.clear(screen.getByLabelText("Password"));
      await user.type(screen.getByLabelText("Password"), "right{Enter}");
      expect(await screen.findByRole("list", { name: "Photos" })).toBeInTheDocument();
      expect(unlock).toHaveBeenLastCalledWith("secret-token", "right");
    });

    it("explains a rate limit", async () => {
      vi.spyOn(shareApi, "publicInfo").mockResolvedValue({ needs_password: true, unlocked: false });
      vi.spyOn(shareApi, "publicUnlock").mockRejectedValue(new ApiError(429, "too many attempts, try again later"));
      const user = userEvent.setup();
      renderView();
      await user.type(await screen.findByLabelText("Password"), "x{Enter}");
      expect(await screen.findByRole("alert")).toHaveTextContent(/Too many attempts/);
    });

    it("goes back to the form when the unlock expires while browsing", async () => {
      vi.spyOn(shareApi, "publicInfo")
        .mockResolvedValueOnce(pubInfo({ needs_password: true }))
        .mockResolvedValue({ needs_password: true, unlocked: false });
      vi.spyOn(shareApi, "publicItems").mockRejectedValue(new ApiError(401, "unauthorized"));
      renderView();
      expect(await screen.findByRole("heading", { name: "This link is protected" })).toBeInTheDocument();
    });
  });

  describe("viewer", () => {
    it("opens a photo with its preview and moves between photos", async () => {
      open();
      const user = userEvent.setup();
      renderView();
      await user.click(await screen.findByRole("button", { name: "Open photo 2 of 3" }));
      const dialog = screen.getByRole("dialog", { name: "Photo" });
      expect(within(dialog).getByText("2 / 3")).toBeInTheDocument();
      expect(dialog.querySelector("img")).toHaveAttribute("src", "https://s3.test/preview-2");
      await user.click(within(dialog).getByRole("button", { name: "Next photo" }));
      expect(within(dialog).getByText("3 / 3")).toBeInTheDocument();
      expect(within(dialog).getByRole("button", { name: "Next photo" })).toBeDisabled();
      await user.keyboard("{ArrowLeft}");
      expect(within(dialog).getByText("2 / 3")).toBeInTheDocument();
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("offers a plain download link through the share's own route when downloads are allowed", async () => {
      open();
      const user = userEvent.setup();
      renderView("tok");
      await user.click(await screen.findByRole("button", { name: "Open photo 1 of 3" }));
      const link = within(screen.getByRole("dialog")).getByRole("link", { name: "Download" });
      expect(link).toHaveAttribute("href", "/assets/public/s/tok/items/p-1/download");
    });

    it("offers no download at all when the owner turned them off", async () => {
      open([pubItem(1, { downloads: [] })], pubInfo({ allow_download: false, item_count: 1 }));
      const user = userEvent.setup();
      renderView();
      await user.click(await screen.findByRole("button", { name: "Open photo 1 of 1" }));
      const dialog = screen.getByRole("dialog");
      expect(within(dialog).queryByRole("link", { name: "Download" })).not.toBeInTheDocument();
      expect(within(dialog).queryByRole("button", { name: /Download/ })).not.toBeInTheDocument();
    });

    it("offers both files of a RAW + JPEG pair only when the server lists both", async () => {
      open(
        [
          pubItem(1, {
            downloads: [
              { id: "p-1", label: "JPEG", size_bytes: 3_000_000 },
              { id: "raw-1", label: "RAW", size_bytes: 25_000_000 },
            ],
          }),
        ],
        pubInfo({ include_originals: true, item_count: 1 }),
      );
      const user = userEvent.setup();
      renderView("tok");
      await user.click(await screen.findByRole("button", { name: "Open photo 1 of 1" }));
      await user.click(screen.getByRole("button", { name: "Download" }));
      expect(screen.getByRole("link", { name: /Download JPEG/ })).toHaveAttribute("href", "/assets/public/s/tok/items/p-1/download");
      expect(screen.getByRole("link", { name: /Download RAW/ })).toHaveAttribute(
        "href",
        "/assets/public/s/tok/items/p-1/download?file=raw-1",
      );
    });

    it("asks for the download copy of a JPEG and enables the link when it is ready", async () => {
      open([pubItem(1, { downloads: [], download_pending: true })], pubInfo({ item_count: 1 }));
      const prepare = vi.spyOn(shareApi, "publicPrepare").mockResolvedValue({ pending: true });
      const poll = vi
        .spyOn(shareApi, "publicItem")
        .mockResolvedValueOnce(pubItem(1, { downloads: [], download_pending: true }))
        .mockResolvedValue(pubItem(1));
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      vi.useFakeTimers({ shouldAdvanceTime: true });
      renderView("tok");
      await user.click(await screen.findByRole("button", { name: "Open photo 1 of 1" }));
      expect(await screen.findByRole("button", { name: "Preparing download…" })).toBeDisabled();
      await waitFor(() => expect(prepare).toHaveBeenCalledWith("tok", "p-1"));
      await act(() => vi.advanceTimersByTimeAsync(800));
      await act(() => vi.advanceTimersByTimeAsync(1500));
      expect(await screen.findByRole("link", { name: "Download" })).toHaveAttribute("href", "/assets/public/s/tok/items/p-1/download");
      expect(poll).toHaveBeenCalledTimes(2);
      expect(screen.queryByRole("button", { name: "Preparing download…" })).not.toBeInTheDocument();
    });

    it("does not prepare anything when downloads are off", async () => {
      open([pubItem(1, { downloads: [], download_pending: false })], pubInfo({ allow_download: false, item_count: 1 }));
      const prepare = vi.spyOn(shareApi, "publicPrepare").mockResolvedValue({ pending: false });
      const user = userEvent.setup();
      renderView();
      await user.click(await screen.findByRole("button", { name: "Open photo 1 of 1" }));
      expect(prepare).not.toHaveBeenCalled();
    });
  });

  it("loads further pages with the cursor when there is no scroll observer", async () => {
    vi.spyOn(shareApi, "publicInfo").mockResolvedValue(pubInfo({ item_count: 3 }));
    const items = vi
      .spyOn(shareApi, "publicItems")
      .mockResolvedValueOnce({ items: [pubItem(1), pubItem(2)], next_cursor: "c1" })
      .mockResolvedValue({ items: [pubItem(3)], next_cursor: null });
    const user = userEvent.setup();
    renderView();
    await user.click(await screen.findByRole("button", { name: "Show more" }));
    await waitFor(() => expect(within(screen.getByRole("list", { name: "Photos" })).getAllByRole("button")).toHaveLength(3));
    expect(items).toHaveBeenLastCalledWith("tok", "c1");
    expect(screen.queryByRole("button", { name: "Show more" })).not.toBeInTheDocument();
  });

  it("says so when the link currently holds no photos", async () => {
    open([], pubInfo({ item_count: 0 }));
    renderView();
    expect(await screen.findByText("Nothing to see here")).toBeInTheDocument();
  });

  beforeEach(() => {
    vi.stubGlobal("IntersectionObserver", undefined);
  });
});
