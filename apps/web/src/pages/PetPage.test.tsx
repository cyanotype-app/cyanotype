import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "../api";
import { AuthProvider } from "../auth";
import { fakeLibrary, type FakeLibrary } from "../test/fakeLibrary";
import { detail, item, namedPet } from "../test/fixtures";
import { PetsProvider } from "../usePets";
import { PetPage } from "./PetPage";

let lib: FakeLibrary;
beforeEach(() => {
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    },
  );
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co", role: "member" });
  vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
  vi.spyOn(api, "pets").mockResolvedValue({ any: 0, kinds: [], scanned: 0, total: 0 });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
  vi.spyOn(api, "petSuggestions").mockResolvedValue({ items: [], counts: { likely: 0, maybe: 0 } });
  lib = fakeLibrary([item(1), item(2), item(3)]);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderAt = (path = "/pets/p1") =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <PetsProvider>
          <Routes>
            <Route path="/pets" element={<p>all pets</p>} />
            <Route path="/pets/:id" element={<PetPage />} />
          </Routes>
        </PetsProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

async function choose(name: string) {
  await userEvent.click(await screen.findByRole("button", { name: "More" }));
  await userEvent.click(await screen.findByRole("menuitem", { name }));
}
const open = (n: number) => screen.getByRole("button", { name: new RegExp(`^(Open|Select) photo-${n}\\.jpg$`) });

describe("a pet's page", () => {
  it("shows the name, the kind, the count, what waits for an answer and the photos, with a way back", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3, { pending: 4 }));
    renderAt();
    expect(await screen.findByRole("heading", { level: 1, name: "Pet A" })).toBeInTheDocument();
    expect(screen.getAllByText("3 photos").length).toBeGreaterThan(0);
    expect(screen.getByText("Dog")).toBeInTheDocument();
    expect(screen.getByText("4 photos to confirm")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Open photo-1.jpg" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Pets" })).toHaveAttribute("href", "/pets");
    expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ pet: "p1" }));
    expect(lib.listAssets).toHaveBeenCalledWith(expect.objectContaining({ pet: "p1" }));
  });

  it("reads the timeline again when photos join the pet in the background", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const get = vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
      renderAt();
      await screen.findByAltText("photo-1.jpg");
      lib.photos.push(item(4, { sort_time: "2026-01-02T00:00:00Z" }));
      get.mockResolvedValue(namedPet("p1", "Pet A", 4));
      await vi.advanceTimersByTimeAsync(15_500);
      expect(await screen.findByAltText("photo-4.jpg")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("selects the whole pet by its scope for a bulk action, and 'Not this pet' needs the photos one by one", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const user = userEvent.setup();
    renderAt();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    expect(screen.getByRole("button", { name: "Not this pet" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Favorite" }));
    await waitFor(() => expect(api.favoriteAssets).toHaveBeenCalledWith({ scope: { pet: "p1" }, except: [] }, true));
  });

  it("saves the pet's photos as an album by the {pet} scope, with a title that is not its name", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const create = vi.spyOn(api, "createAlbum").mockResolvedValue({ id: "al1", title: "Holiday", description: null, photo_count: 0, cover: null, first_at: null, last_at: null, created_at: "", updated_at: "" });
    const add = vi.spyOn(api, "addToAlbum").mockResolvedValue({ count: 3 });
    const user = userEvent.setup();
    renderAt();
    await screen.findByRole("heading", { name: "Pet A" });
    await choose("Save as album");
    const dialog = screen.getByRole("dialog", { name: "Save 3 photos as an album" });
    const title = within(dialog).getByLabelText("Album title");
    expect(title).toHaveValue("");
    await user.type(title, "Holiday");
    await user.click(within(dialog).getByRole("button", { name: "Create album" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ title: "Holiday" }));
    expect(add).toHaveBeenCalledWith("al1", { scope: { pet: "p1" }, except: [] });
  });

  it("links to the search with the pet as a filter", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "More" }));
    expect(await screen.findByRole("menuitem", { name: "Search with filters" })).toHaveAttribute("href", "/search?pet=p1");
  });

  it("says so for a pet that is not there, and for the feature being off", async () => {
    vi.spyOn(api, "namedPet").mockRejectedValue(new ApiError(404, "not found"));
    renderAt();
    expect(await screen.findByRole("heading", { name: "Pet not found" })).toBeInTheDocument();
    vi.restoreAllMocks();
    vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co", role: "member" });
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: false });
    const get = vi.spyOn(api, "namedPet");
    renderAt();
    expect(await screen.findAllByRole("heading", { name: "Page not found" })).not.toHaveLength(0);
    expect(get).not.toHaveBeenCalled();
  });

  it("renames in place, and a pet is never without a name", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const update = vi.spyOn(api, "updatePet").mockResolvedValue(namedPet("p1", "Pet B", 3));
    const user = userEvent.setup();
    renderAt();
    await choose("Rename");
    const input = screen.getByLabelText("Name");
    expect(input).toHaveValue("Pet A");
    await user.clear(input);
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Give your pet a name.")).toBeInTheDocument();
    expect(update).not.toHaveBeenCalled();
    await user.type(input, "  Pet B ");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(update).toHaveBeenCalledWith("p1", { name: "Pet B" });
    expect(await screen.findByRole("heading", { level: 1, name: "Pet B" })).toBeInTheDocument();
  });

  it("changes nothing when a rename is cancelled", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const update = vi.spyOn(api, "updatePet");
    renderAt();
    await choose("Rename");
    await userEvent.type(screen.getByLabelText("Name"), " and more");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(update).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { level: 1, name: "Pet A" })).toBeInTheDocument();
  });

  it("hides and shows the pet", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const update = vi.spyOn(api, "updatePet").mockResolvedValue(namedPet("p1", "Pet A", 3, { hidden: true }));
    renderAt();
    await choose("Hide");
    expect(update).toHaveBeenCalledWith("p1", { hidden: true });
    await userEvent.click(await screen.findByRole("button", { name: "More" }));
    expect(await screen.findByRole("menuitem", { name: "Show" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.getByText(/^Hidden:/)).toBeInTheDocument();
  });

  it("takes selected photos out with 'Not this pet', after asking", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const remove = vi.spyOn(api, "removeFromPet").mockResolvedValue({ removed: 2 });
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Select" }));
    await userEvent.click(open(1));
    await userEvent.click(open(3));
    await userEvent.click(screen.getByRole("button", { name: "Not this pet" }));
    const dialog = await screen.findByRole("dialog", { name: "Not Pet A?" });
    expect(dialog).toHaveTextContent("taken out of 2 photos");
    await userEvent.click(within(dialog).getByRole("button", { name: "Not this pet" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("p1", { asset_ids: ["id-1", "id-3"] }));
    expect(await screen.findByText("Took 2 photos out of Pet A")).toBeInTheDocument();
    await waitFor(() => expect(lib.timeline).toHaveBeenCalledTimes(2));
  });

  it("makes one selected photo the cover through that photo's animal of the pet", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    vi.spyOn(api, "assetPets").mockResolvedValue({
      scanned: true,
      items: [
        { id: "d-other", kind: "dog", box: [0, 0, 0.1, 0.1], score: 0.9, pet: { id: "p9", name: "Pet Z" } },
        { id: "d-mine", kind: "dog", box: [0.5, 0.5, 0.1, 0.1], score: 0.9, pet: { id: "p1", name: "Pet A" } },
      ],
    });
    const update = vi.spyOn(api, "updatePet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Select" }));
    const cover = screen.getByRole("button", { name: "Use as cover" });
    expect(cover).toBeDisabled();
    await userEvent.click(open(2));
    await userEvent.click(cover);
    await waitFor(() => expect(update).toHaveBeenCalledWith("p1", { cover_detection_id: "d-mine" }));
    expect(api.assetPets).toHaveBeenCalledWith("id-2");
  });

  it("changes the cover to one of the pet's animals, chosen from their crops", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const dets = [1, 2, 3].map((n) => ({ detection_id: `cd${n}`, asset_id: `a${n}`, box: [0.1, 0.1, 0.2, 0.2] as [number, number, number, number], preview_url: `https://s3.test/c${n}` }));
    vi.spyOn(api, "petDetections").mockResolvedValue({ items: dets, next_cursor: null });
    const update = vi.spyOn(api, "updatePet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Change cover" }));
    const dialog = await screen.findByRole("dialog", { name: "Change cover of Pet A" });
    const list = await within(dialog).findByRole("list", { name: "Animals" });
    expect(within(list).getAllByRole("button")).toHaveLength(3);
    await userEvent.click(within(list).getByRole("button", { name: "Animal 2" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith("p1", { cover_detection_id: "cd2" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByText("Cover changed")).toBeInTheDocument();
  });

  it("merges into another pet of the same kind, never into itself or another kind", async () => {
    vi.spyOn(api, "namedPet").mockImplementation(async (id) => namedPet(id, id === "p1" ? "Pet A" : "Pet B", 3));
    vi.spyOn(api, "namedPets").mockResolvedValue({
      items: [namedPet("p1", "Pet A", 3), namedPet("p2", "Pet B", 5), namedPet("p3", "Pet C", 2, { kind: "cat" })],
    });
    const merge = vi.spyOn(api, "mergePet").mockResolvedValue({});
    renderAt();
    await screen.findByRole("heading", { name: "Pet A" });
    await choose("Merge into…");
    const choices = await screen.findByRole("group", { name: "Merge into" });
    await waitFor(() => expect(within(choices).getAllByRole("radio")).toHaveLength(1));
    expect(within(choices).getByRole("radio", { name: /Pet B/ })).toBeChecked();
    await userEvent.click(screen.getByRole("button", { name: "Merge" }));
    await waitFor(() => expect(merge).toHaveBeenCalledWith("p1", "p2"));
    await waitFor(() => expect(api.namedPet).toHaveBeenCalledWith("p2"));
  });

  it("explains when there is nothing to merge into", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    vi.spyOn(api, "namedPets").mockResolvedValue({ items: [namedPet("p1", "Pet A", 3), namedPet("p3", "Pet C", 2, { kind: "cat" })] });
    renderAt();
    await choose("Merge into…");
    expect(await screen.findByText(/no other dog to merge into/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Merge" })).toBeDisabled();
  });

  it("deletes the pet after asking, and goes back to Pets", async () => {
    vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3));
    const del = vi.spyOn(api, "deletePet").mockResolvedValue(undefined);
    renderAt();
    await choose("Delete pet");
    const dialog = await screen.findByRole("dialog", { name: "Delete Pet A?" });
    expect(dialog).toHaveTextContent("The photos stay in your library");
    expect(del).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete pet" }));
    expect(await screen.findByText("all pets")).toBeInTheDocument();
    expect(del).toHaveBeenCalledWith("p1");
  });

  it("reviews the proposals on the page, and reads the pet again after an answer", async () => {
    const get = vi.spyOn(api, "namedPet").mockResolvedValue(namedPet("p1", "Pet A", 3, { pending: 1 }));
    vi.spyOn(api, "petSuggestions").mockResolvedValue({
      items: [{ detection_id: "d1", asset_id: "a1", box: [0.1, 0.1, 0.3, 0.3], preview_url: "https://s3.test/p1", kind: "dog", similarity: 0.8, tier: "likely" }],
      counts: { likely: 1, maybe: 0 },
    });
    vi.spyOn(api, "answerPetSuggestions").mockResolvedValue({ confirmed: 1, rejected: 0, skipped: 0 });
    renderAt();
    expect(await screen.findByRole("heading", { name: "Is this Pet A?" })).toBeInTheDocument();
    get.mockClear();
    await userEvent.click(screen.getByRole("button", { name: "Yes, this is Pet A" }));
    await waitFor(() => expect(get).toHaveBeenCalled());
  });
});
