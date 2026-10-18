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
    expect(screen.getByText("2 matches")).toBeInTheDocument();
    expect(search).toHaveBeenCalledWith({ q: "dog on the beach" }, { cursor: null, limit: 60, weaker: false });
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
    await screen.findByText(/matches so far/);
    await act(async () => io([{ isIntersecting: true }]));
    await screen.findByText("3 matches");
    expect(search).toHaveBeenLastCalledWith({ q: "cat" }, { cursor: "c1", limit: 60, weaker: false });
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
    await waitFor(() =>
      expect(search).toHaveBeenLastCalledWith({ type: "raw", from: "2024-01-01", to: "2024-12-31" }, { cursor: null, limit: 60, weaker: false }),
    );
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

  it("offers the weaker matches behind the strong ones and continues where the strong ones ended", async () => {
    const strong = (n: number): SearchItem => ({ ...hit(n), tier: "strong" });
    const weak = (n: number): SearchItem => ({ ...hit(n, 0.1), tier: "weak" });
    const search = vi
      .spyOn(api, "search")
      .mockResolvedValueOnce(result([strong(1), strong(2)], { strong_count: 2, weaker: { count: 2, cursor: "after-strong" } }))
      .mockResolvedValueOnce(result([weak(3), weak(4)]));
    renderAt("/search?q=cat");
    expect(await screen.findByText("2 matches")).toBeInTheDocument();
    const more = screen.getByRole("button", { name: "Show weaker matches (2)" });
    await userEvent.click(more);
    await screen.findByText("2 matches, 2 weaker");
    expect(search).toHaveBeenLastCalledWith({ q: "cat" }, { cursor: "after-strong", limit: 60, weaker: true });
    expect(screen.queryByRole("button", { name: /Show weaker matches/ })).not.toBeInTheDocument();
    expect(screen.getAllByRole("listitem").length).toBeGreaterThanOrEqual(4);
  });

  it("with no strong matches says so, and the weaker ones are one click away", async () => {
    const search = vi
      .spyOn(api, "search")
      .mockResolvedValueOnce(result([], { strong_count: 0, weaker: { count: 5, cursor: null } }))
      .mockResolvedValueOnce(result([{ ...hit(7, 0.09), tier: "weak" }, { ...hit(8, 0.09), tier: "weak" }]));
    renderAt("/search?q=giraffe");
    expect(await screen.findByRole("heading", { name: "No close matches" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show weaker matches (5)" }));
    await screen.findByText("0 matches, 2 weaker");
    expect(search).toHaveBeenLastCalledWith({ q: "giraffe" }, { cursor: null, limit: 60, weaker: true });
    expect(screen.queryByRole("heading", { name: "No close matches" })).not.toBeInTheDocument();
  });

  it("shows what was read out of the query as chips, and removing one searches again with it left in the text", async () => {
    const parsed = {
      text: "dog",
      chips: [
        { kind: "date" as const, label: "June 2023", matched: "in june 2023", from: "2023-06-01", to: "2023-06-30" },
        { kind: "place" as const, label: "Maui", matched: "Maui", value: "Maui", place_kind: "city" as const },
      ],
    };
    const search = vi.spyOn(api, "search").mockResolvedValue(result([hit(1)], { parsed, strong_count: 1 }));
    renderAt("/search?q=dog%20Maui%20in%20june%202023");
    const chips = await screen.findByRole("list", { name: "Understood from your search" });
    expect(within(chips).getByText("June 2023")).toBeInTheDocument();
    expect(within(chips).getByText("Maui")).toBeInTheDocument();
    await userEvent.click(within(chips).getByRole("button", { name: "Remove date June 2023" }));
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent("skip=date"));
    expect(screen.getByTestId("where")).toHaveTextContent("q=dog+Maui+in+june+2023");
    await waitFor(() =>
      expect(search).toHaveBeenLastCalledWith({ q: "dog Maui in june 2023", skip: "date" }, { cursor: null, limit: 60, weaker: false }),
    );
  });

  it("selecting all of a filter-only search that was read out of the query keeps the dates and places it was read as", async () => {
    const parsed = { text: "", chips: [{ kind: "date" as const, label: "2023", matched: "from 2023", from: "2023-01-01", to: "2023-12-31" }] };
    vi.spyOn(api, "search").mockResolvedValue(result([hit(1, null), hit(2, null)], { ranked: false, total: 900, parsed }));
    const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 900 });
    renderAt("/search?q=photos%20from%202023");
    await screen.findByRole("list", { name: "Photos" });
    await userEvent.click(screen.getByRole("button", { name: "Select" }));
    await userEvent.click(screen.getByRole("button", { name: "Select all" }));
    await userEvent.click(screen.getByRole("button", { name: "Favorite" }));
    await waitFor(() => expect(fav).toHaveBeenCalled());
    expect(fav).toHaveBeenCalledWith({ scope: { search: { from: "2023-01-01", to: "2023-12-31" } }, except: [] }, true);
  });

  it("says a re-index is under way while an older model answers, instead of 'results may be incomplete'", async () => {
    vi.spyOn(api, "search").mockResolvedValue(
      result([hit(1)], { reindexing: { model: "siglip2-b16", indexed: 400, total: 1000, percent: 40, switch_at_percent: 95 } }),
    );
    vi.spyOn(api, "processing").mockResolvedValue({
      ...idle,
      embedding: {
        indexed: 400, total: 1000, queued: 600, running: 0, failed: 0,
        model: "siglip2-b16", percent: 40, reindexing: true, switch_at_percent: 95, serving_model: "clip-b32",
      },
    });
    renderAt("/search?q=cat");
    expect(await screen.findAllByText(/Re-indexing with the improved search model: 40%/)).not.toHaveLength(0);
    expect(screen.queryByText(/results may be incomplete/)).not.toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Search re-indexing progress" })).toHaveAttribute("aria-valuenow", "40");
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
  describe("More like this", () => {
    const src = "6f1c2b0e-0000-4000-8000-000000000007";

    it("lists photos like the source, shows its chip, and removing the chip leaves the mode", async () => {
      const search = vi.spyOn(api, "search").mockResolvedValue(result([hit(1, 0.9), hit(2, 0.8)], { similar_to: src }));
      renderAt(`/search?similar=${src}&favorite=true`);
      expect(await screen.findByText("2 similar photos")).toBeInTheDocument();
      expect(search).toHaveBeenCalledWith({ similar: src, favorite: true }, { cursor: null, limit: 60, weaker: false });
      expect(screen.getByText(/Similar to photo-0.jpg/)).toBeInTheDocument();
      // The filters still work next to it: they keep the source in the URL.
      expect(within(screen.getByRole("list", { name: "Active filters" })).getByText("Favorites")).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Remove similar photo" }));
      await waitFor(() => expect(screen.getByTestId("where")).not.toHaveTextContent("similar"));
      expect(screen.getByTestId("where")).toHaveTextContent("favorite=true");
    });

    it("selects what is loaded (not a scope) with Select all", async () => {
      vi.spyOn(api, "search").mockResolvedValue(result([hit(1, 0.9), hit(2, 0.8)], { similar_to: src, total: null }));
      const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 2 });
      renderAt(`/search?similar=${src}`);
      await screen.findByText("2 similar photos");
      await userEvent.click(screen.getByRole("button", { name: "Select" }));
      await userEvent.click(screen.getByRole("button", { name: /Select all/ }));
      await userEvent.click(screen.getByRole("button", { name: "Favorite" }));
      await waitFor(() => expect(fav).toHaveBeenCalledWith(["id-1", "id-2"], true));
    });

    it("says the photo is still being indexed when the server answers 409", async () => {
      vi.spyOn(api, "search").mockRejectedValue(new ApiError(409, "this photo is still being indexed: similar photos are available once it is"));
      renderAt(`/search?similar=${src}`);
      expect(await screen.findByRole("heading", { name: "This photo is still being indexed" })).toBeInTheDocument();
      expect(screen.queryByText("Couldn’t search")).not.toBeInTheDocument();
    });
  });
  describe("bursts", () => {
    const withBurst = () => [
      hit(1),
      { ...hit(2), burst: { size: 3, members: [{ ...item(3), score: 0.5 }, { ...item(4), score: 0.4 }] } },
    ];

    it("shows a burst as one tile with a +N badge that opens its photos inline", async () => {
      vi.spyOn(api, "search").mockResolvedValue(result(withBurst()));
      renderAt("/search?q=cat");
      await screen.findByRole("button", { name: "Open photo-2.jpg" });
      expect(screen.queryByRole("button", { name: "Open photo-3.jpg" })).not.toBeInTheDocument();
      expect(screen.getByText("2 matches")).toBeInTheDocument(); // results, not photos
      const badge = screen.getByRole("button", { name: "Show 2 more photos from this burst" });
      expect(badge).toHaveTextContent("+2");
      await userEvent.click(badge);
      expect(screen.getByRole("button", { name: "Open photo-3.jpg" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Open photo-4.jpg" })).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Hide the other 2 photos of this burst" }));
      expect(screen.queryByRole("button", { name: "Open photo-3.jpg" })).not.toBeInTheDocument();
    });

    it("selects every photo of a collapsed burst with one tap, and says so", async () => {
      vi.spyOn(api, "search").mockResolvedValue(result(withBurst()));
      const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 3 });
      renderAt("/search?q=cat");
      await screen.findByRole("button", { name: "Open photo-2.jpg" });
      await userEvent.click(screen.getByRole("button", { name: "Select" }));
      expect(screen.getByRole("button", { name: "Select photo-2.jpg and 2 more from its burst" })).toBeInTheDocument();
      expect(screen.getByText(/2 more in 1 burst/)).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Select photo-2.jpg and 2 more from its burst" }));
      expect(screen.getByText("3 selected")).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Favorite" }));
      await waitFor(() => expect(fav).toHaveBeenCalledWith(["id-2", "id-3", "id-4"], true));
    });

    it("selects one photo at a time once the burst is open, and Select all takes every photo of the results", async () => {
      vi.spyOn(api, "search").mockResolvedValue(result(withBurst()));
      const trash = vi.spyOn(api, "trashAssets").mockResolvedValue({ count: 4 });
      renderAt("/search?q=cat");
      await screen.findByRole("button", { name: "Open photo-2.jpg" });
      await userEvent.click(screen.getByRole("button", { name: "Show 2 more photos from this burst" }));
      await userEvent.click(screen.getByRole("button", { name: "Select" }));
      await userEvent.click(screen.getByRole("button", { name: "Select photo-3.jpg" }));
      expect(screen.getByText("1 selected")).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Hide the other 2 photos of this burst" }));
      await userEvent.click(screen.getByRole("button", { name: /Select all/ }));
      expect(screen.getByText("4 selected")).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Move to trash" }));
      await waitFor(() => expect(trash).toHaveBeenCalledWith(["id-1", "id-2", "id-3", "id-4"]));
    });
  });
});
