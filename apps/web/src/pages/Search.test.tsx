import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type Processing, type SearchItem, type SearchResult } from "../api";
import { AuthProvider } from "../auth";
import { UploadProvider } from "../upload/UploadProvider";
import { detail, item } from "../test/fixtures";
import { SearchPage } from "./Search";

const hit = (n: number, score: number | null = 0.3): SearchItem => ({ ...item(n), score });
const result = (items: SearchItem[], over: Partial<SearchResult> = {}): SearchResult => ({
  items,
  next_cursor: null,
  ranked: true,
  total: null,
  ...over,
});
const idle: Processing = { active: false, queued: 0, running: 0, failed: 0, by_kind: [], total_photos: 10, without_thumbnail: 0, failures: [] };

beforeEach(() => {
  localStorage.clear();
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "processing").mockResolvedValue(idle);
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
  vi.spyOn(api, "getAlbum").mockRejectedValue(new ApiError(404, "not found"));
});
afterEach(() => vi.restoreAllMocks());

function Where() {
  const l = useLocation();
  return <p data-testid="where">{l.pathname + l.search}</p>;
}
const renderAt = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <Routes>
            <Route path="/search" element={<SearchPage />} />
          </Routes>
          <Where />
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

describe("Search page", () => {
  it("without a query or filters shows how to search, examples and recent searches, and does not call the server", async () => {
    localStorage.setItem("cyanotype.recentSearches", JSON.stringify(["red car"]));
    const search = vi.spyOn(api, "search");
    renderAt("/search");
    expect(await screen.findByRole("heading", { name: "Search your photos" })).toBeInTheDocument();
    await userEvent.click(within(screen.getByRole("list", { name: "Recent searches" })).getByRole("button", { name: "red car" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/search?q=red+car");
    expect(search).toHaveBeenCalledTimes(1); // the recent search itself, not the empty page
  });

  it("shows ranked results with a count, and requests the query as typed", async () => {
    const search = vi.spyOn(api, "search").mockResolvedValue(result([hit(1), hit(2)]));
    renderAt("/search?q=dog%20on%20the%20beach");
    expect(await screen.findByRole("list", { name: "Photos" })).toBeInTheDocument();
    expect(screen.getAllByRole("listitem").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText("2 best matches")).toBeInTheDocument();
    expect(search).toHaveBeenCalledWith({ q: "dog on the beach" }, { cursor: null, limit: 60 });
  });

  it("pages with the cursor when scrolled, and never repeats a photo", async () => {
    let io: (e: { isIntersecting: boolean }[]) => void = () => undefined;
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
    const search = vi
      .spyOn(api, "search")
      .mockResolvedValueOnce(result([hit(1), hit(2)], { next_cursor: "c1" }))
      .mockResolvedValueOnce(result([hit(2), hit(3)]));
    renderAt("/search?q=cat");
    await screen.findByText(/best matches so far/);
    await act(async () => io([{ isIntersecting: true }]));
    await screen.findByText("3 best matches");
    expect(search).toHaveBeenLastCalledWith({ q: "cat" }, { cursor: "c1", limit: 60 });
    vi.unstubAllGlobals();
  });

  it("shows a chip per filter and removing one searches again without it", async () => {
    const search = vi.spyOn(api, "search").mockResolvedValue(result([hit(1)], { ranked: false, total: 1 }));
    renderAt("/search?favorite=true&type=raw&from=2024-01-01&to=2024-12-31");
    const chips = await screen.findByRole("list", { name: "Active filters" });
    expect(within(chips).getByText("2024")).toBeInTheDocument();
    expect(within(chips).getByText("Favorites")).toBeInTheDocument();
    expect(within(chips).getByText("RAW")).toBeInTheDocument();
    expect(screen.getByText("1 photo")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Remove filter Favorites" }));
    await waitFor(() => expect(screen.getByTestId("where")).not.toHaveTextContent("favorite"));
    expect(screen.getByTestId("where")).toHaveTextContent("type=raw");
    await waitFor(() => expect(search).toHaveBeenLastCalledWith({ type: "raw", from: "2024-01-01", to: "2024-12-31" }, { cursor: null, limit: 60 }));
  });

  it("says nothing matched, and that unindexed photos cannot match yet while indexing", async () => {
    vi.spyOn(api, "search").mockResolvedValue(result([]));
    vi.spyOn(api, "processing").mockResolvedValue({
      ...idle,
      active: false,
      embedding: { indexed: 10, total: 50, queued: 40, running: 0, failed: 0 },
    });
    renderAt("/search?q=unicorn");
    expect(await screen.findByRole("heading", { name: "Nothing matched" })).toBeInTheDocument();
    expect(await screen.findAllByText("Still indexing 40 photos: results may be incomplete.")).not.toHaveLength(0);
    expect(screen.getByText(/still being indexed cannot match a description yet/)).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Search indexing progress" })).toHaveAttribute("aria-valuenow", "20");
  });

  it("explains a server without the search model, and keeps filters usable", async () => {
    vi.spyOn(api, "search").mockRejectedValue(new ApiError(503, "search is not available: the model files are not installed"));
    renderAt("/search?q=cat");
    expect(await screen.findByRole("heading", { name: "Search isn’t available" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Filters/ })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Try again" })).not.toBeInTheDocument();
  });

  it("select all on a filter-only search is the whole search (a scope), minus what was deselected", async () => {
    vi.spyOn(api, "search").mockResolvedValue(result([hit(1, null), hit(2, null), hit(3, null)], { ranked: false, total: 1200 }));
    const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 1199 });
    renderAt("/search?camera=Canon%20EOS%20R5&favorite=false");
    await screen.findByRole("list", { name: "Photos" });
    await userEvent.click(screen.getByRole("button", { name: "Select" }));
    await userEvent.click(screen.getByRole("button", { name: "Select all" }));
    expect(screen.getByText("1,200 selected")).toBeInTheDocument();
    await userEvent.click(screen.getAllByRole("button", { name: /photo-1\.jpg/ })[0]!); // deselect one
    expect(screen.getByText("1,199 selected")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Favorite" }));
    await waitFor(() => expect(fav).toHaveBeenCalled());
    expect(fav).toHaveBeenCalledWith({ scope: { search: { camera: "Canon EOS R5" } }, except: ["id-1"] }, true);
  });

  it("select all on a ranked search selects the loaded results by id (a text query cannot be a scope)", async () => {
    vi.spyOn(api, "search").mockResolvedValue(result([hit(1), hit(2)]));
    const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 2 });
    renderAt("/search?q=cat");
    await screen.findByRole("list", { name: "Photos" });
    await userEvent.click(screen.getByRole("button", { name: "Select" }));
    await userEvent.click(screen.getByRole("button", { name: "Select all" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Favorite" }));
    await waitFor(() => expect(fav).toHaveBeenCalledWith(["id-1", "id-2"], true));
  });
});
