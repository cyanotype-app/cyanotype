// The pets of the open photo, in the viewer's Info panel: the named pet each animal is in, "Not Name", and "Add to pet…" for one nobody named.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type PhotoPet } from "../api";
import { bareDetails, detail, item, namedPet } from "../test/fixtures";
import { PetsProvider } from "../usePets";
import { Lightbox } from "./Lightbox";

vi.mock("../map/PlaceMap", () => ({ default: () => <div data-testid="place-map" /> }));

const items = [1, 2].map((n) => item(n));
const animal = (id: string, kind: "dog" | "cat", pet: { id: string; name: string } | null): PhotoPet => ({ id, kind, box: [0.1, 0.1, 0.3, 0.3], score: 0.9, pet });

function Where() {
  const l = useLocation();
  return <p data-testid="where">{l.pathname}</p>;
}

function renderViewer(onClose = vi.fn()) {
  render(
    <MemoryRouter initialEntries={["/photos"]}>
      <PetsProvider>
        <Routes>
          <Route
            path="*"
            element={
              <>
                <Lightbox
                  items={items}
                  index={0}
                  onIndexChange={() => {}}
                  onClose={onClose}
                  loadDetail={async (id) => detail(Number(id.split("-")[1]))}
                  loadDetails={async (id) => ({ ...bareDetails(), id })}
                />
                <Where />
              </>
            }
          />
        </Routes>
      </PetsProvider>
    </MemoryRouter>,
  );
  return onClose;
}

beforeEach(async () => {
  await new Promise((r) => setTimeout(r, 20));
  window.history.replaceState(null, "");
  vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
  vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
  vi.spyOn(api, "pets").mockResolvedValue({ any: 2, kinds: [], scanned: 2, total: 2 });
  vi.spyOn(api, "namedPets").mockResolvedValue({ items: [namedPet("p1", "Pet A", 3), namedPet("p2", "Pet B", 2, { kind: "cat" })] });
});
afterEach(() => vi.restoreAllMocks());

const panel = async () => {
  await userEvent.keyboard("i");
  return screen.findByRole("dialog", { name: "Info" });
};

describe("the viewer's Pets row", () => {
  it("names the pet an animal is in, linking to its page, and offers 'Not Pet A'", async () => {
    vi.spyOn(api, "assetPets").mockResolvedValue({ scanned: true, items: [animal("d1", "dog", { id: "p1", name: "Pet A" })] });
    const remove = vi.spyOn(api, "removeFromPet").mockResolvedValue({ removed: 1 });
    const onClose = renderViewer();
    const info = await panel();
    const link = await within(info).findByRole("link", { name: "Pet A" });
    expect(link).toHaveAttribute("href", "/pets/p1");
    // After the taking out, the animal is not in the pet any more.
    vi.mocked(api.assetPets).mockResolvedValue({ scanned: true, items: [animal("d1", "dog", null)] });
    await userEvent.click(within(info).getByRole("button", { name: "Not Pet A" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("p1", { detection_ids: ["d1"] }));
    expect(await screen.findByText("Took 1 photo out of Pet A")).toBeInTheDocument();
    expect(await within(info).findByRole("button", { name: "Add to pet…" })).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("puts an animal nobody named in a pet that is there already", async () => {
    vi.spyOn(api, "assetPets").mockResolvedValue({ scanned: true, items: [animal("d1", "dog", null)] });
    const add = vi.spyOn(api, "addToPet").mockResolvedValue({ pet: namedPet("p1", "Pet A", 4), assigned: 1, ambiguous: [], skipped: [] });
    renderViewer();
    const info = await panel();
    expect(await within(info).findByText("not named")).toBeInTheDocument();
    await userEvent.click(within(info).getByRole("button", { name: "Add to pet…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add to pet" });
    // Every pet is offered (the detector can be wrong about the kind), and a new pet starts as the animal's kind.
    const choose = await within(dialog).findByLabelText("Pet");
    expect(within(choose).getAllByRole("option").map((o) => o.textContent)).toEqual(["A new pet", "Pet A (dog)", "Pet B (cat)"]);
    expect(within(dialog).getByRole("radio", { name: "Dog" })).toBeChecked();
    await userEvent.selectOptions(choose, "p1");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add photos" }));
    await waitFor(() => expect(add).toHaveBeenCalledWith("p1", { detection_ids: ["d1"] }));
    expect(await screen.findByText("Added 1 photo to Pet A")).toBeInTheDocument();
  });

  it("makes a new pet of an animal, and the toast opens its page", async () => {
    vi.spyOn(api, "assetPets").mockResolvedValue({ scanned: true, items: [animal("d1", "cat", null)] });
    const create = vi.spyOn(api, "createPet").mockResolvedValue({ pet: namedPet("p9", "Pet C", 1, { kind: "cat" }), assigned: 1, ambiguous: [], skipped: [] });
    const onClose = renderViewer();
    const info = await panel();
    await userEvent.click(await within(info).findByRole("button", { name: "Add to pet…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add to pet" });
    await userEvent.type(within(dialog).getByLabelText("Name"), "Pet C");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ name: "Pet C", kind: "cat", detection_ids: ["d1"] }));
    await userEvent.click(await screen.findByRole("button", { name: "Open" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent("/pets/p9"));
  });

  it("numbers the animals when there are several, with buttons that say which", async () => {
    vi.spyOn(api, "assetPets").mockResolvedValue({ scanned: true, items: [animal("d1", "dog", { id: "p1", name: "Pet A" }), animal("d2", "dog", null)] });
    renderViewer();
    const info = await panel();
    expect(await within(info).findByRole("button", { name: "Not Pet A, dog 1" })).toBeInTheDocument();
    expect(within(info).getByRole("button", { name: "Add to pet…, dog 2" })).toBeInTheDocument();
  });

  it("lets the owner say what an animal is: 'It's a dog' on one the detector called a cat", async () => {
    vi.spyOn(api, "assetPets").mockResolvedValue({ scanned: true, items: [animal("d1", "cat", null)] });
    const set = vi.spyOn(api, "setPetDetectionKind").mockResolvedValue({ id: "d1", kind: "dog" });
    renderViewer();
    const info = await panel();
    expect(await within(info).findByText(/Cat/)).toBeInTheDocument();
    vi.mocked(api.assetPets).mockResolvedValue({ scanned: true, items: [animal("d1", "dog", null)] });
    await userEvent.click(within(info).getByRole("button", { name: "More for Cat" }));
    expect(screen.queryByRole("menuitem", { name: "It’s a cat" })).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole("menuitem", { name: "It’s a dog" }));
    await waitFor(() => expect(set).toHaveBeenCalledWith("d1", "dog"));
    expect(await screen.findByText("Marked as a dog")).toBeInTheDocument();
    expect(await within(info).findByText(/Dog/)).toBeInTheDocument();
  });

  it("has no Pets row for a photo without animals, and asks nothing while Pets is off", async () => {
    const photos = vi.spyOn(api, "assetPets").mockResolvedValue({ scanned: true, items: [] });
    renderViewer();
    const info = await panel();
    await waitFor(() => expect(photos).toHaveBeenCalledWith("id-1"));
    expect(within(info).queryByText("Pets")).not.toBeInTheDocument();
  });
});

describe("with Pets off", () => {
  it("does not read the animals of the photo", async () => {
    vi.mocked(api.petsStatus).mockResolvedValue({ enabled: false });
    const photos = vi.spyOn(api, "assetPets");
    renderViewer();
    await panel();
    await waitFor(() => expect(api.petsStatus).toHaveBeenCalled());
    expect(photos).not.toHaveBeenCalled();
  });
});
