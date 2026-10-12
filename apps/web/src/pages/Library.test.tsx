import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
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
        <Library />
      </AuthProvider>
    </MemoryRouter>,
  );

describe("Library", () => {
  it("explains CLI uploads when empty", async () => {
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [], next_before: null });
    renderLibrary();
    expect(await screen.findByText(/uploaded with the command-line tool/i)).toBeInTheDocument();
  });

  it("renders a thumbnail grid, pages with `before` on scroll, and opens the lightbox with the keyboard", async () => {
    const list = vi
      .spyOn(api, "listAssets")
      .mockResolvedValueOnce({ items: [item(1), item(2)], next_before: "cursor-1" })
      .mockResolvedValueOnce({ items: [item(3)], next_before: null });
    const user = userEvent.setup();
    renderLibrary();

    expect(await screen.findByAltText("photo-1.jpg")).toHaveAttribute("src", "https://s3.test/thumb-1");
    expect(list).toHaveBeenCalledTimes(1);

    // The sentinel scrolls into view: the next page is requested with the cursor.
    await act(async () => observers.at(-1)!.cb([{ isIntersecting: true }]));
    await screen.findByAltText("photo-3.jpg");
    expect(list).toHaveBeenLastCalledWith({ before: "cursor-1", limit: 60 });

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

  it("shows a placeholder for photos still being processed", async () => {
    vi.spyOn(api, "listAssets").mockResolvedValue({ items: [item(1, { thumb_url: null })], next_before: null });
    renderLibrary();
    expect(await screen.findByText("Processing…")).toBeInTheDocument();
  });
});
