import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type PetAdded } from "../api";
import { namedPet } from "../test/fixtures";
import { NamePetSheet, petNamedToast } from "./NamePetSheet";

beforeEach(() => {
  vi.spyOn(api, "namedPets").mockResolvedValue({ items: [] });
});
afterEach(() => vi.restoreAllMocks());

const added = (over: Partial<PetAdded> = {}): PetAdded => ({ pet: namedPet("p1", "Pet A"), assigned: 2, ambiguous: [], skipped: [], ...over });

describe("NamePetSheet", () => {
  it("makes a pet from the photos with a name and a kind", async () => {
    const create = vi.spyOn(api, "createPet").mockResolvedValue(added());
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1", "a2"]} onClose={vi.fn()} onDone={done} />);
    const dialog = await screen.findByRole("dialog", { name: "Name pet" });
    await user.type(within(dialog).getByLabelText("Name"), "  Pet A ");
    await user.click(within(dialog).getByRole("radio", { name: "Cat" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(done).toHaveBeenCalled());
    expect(create).toHaveBeenCalledWith({ name: "Pet A", kind: "cat", asset_ids: ["a1", "a2"] });
    expect(done).toHaveBeenCalledWith({ pet: expect.objectContaining({ id: "p1" }), name: "Pet A", assigned: 2, made: true });
  });

  it("needs a name before it asks the server", async () => {
    const create = vi.spyOn(api, "createPet");
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1"]} onClose={vi.fn()} onDone={vi.fn()} />);
    await user.click(await screen.findByRole("button", { name: "Save" }));
    expect(await screen.findByText("Give your pet a name.")).toBeInTheDocument();
    expect(create).not.toHaveBeenCalled();
  });

  it("shows what the server said when it refuses, and stays open", async () => {
    vi.spyOn(api, "createPet").mockRejectedValue(new ApiError(400, "at most 500 pets"));
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1"]} onClose={vi.fn()} onDone={done} />);
    await user.type(await screen.findByLabelText("Name"), "Pet A");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/at most 500 pets/)).toBeInTheDocument();
    expect(done).not.toHaveBeenCalled();
  });

  it("adds the photos to a pet that is there already (no name, no kind to ask)", async () => {
    vi.spyOn(api, "namedPets").mockResolvedValue({ items: [namedPet("p1", "Pet A"), namedPet("p2", "Pet B", 2, { hidden: true })] });
    const add = vi.spyOn(api, "addToPet").mockResolvedValue(added({ assigned: 1 }));
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1"]} onClose={vi.fn()} onDone={done} />);
    const pick = await screen.findByLabelText("Pet");
    // A hidden pet is not offered.
    expect(within(pick).getAllByRole("option").map((o) => o.textContent)).toEqual(["A new pet", "Pet A (dog)"]);
    await user.selectOptions(pick, "p1");
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add photos" }));
    await waitFor(() => expect(done).toHaveBeenCalled());
    expect(add).toHaveBeenCalledWith("p1", { asset_ids: ["a1"] });
    expect(done).toHaveBeenCalledWith({ pet: expect.objectContaining({ id: "p1" }), name: "Pet A", assigned: 1, made: false });
  });

  it("asks which animal when a photo has several, then adds the one that was tapped", async () => {
    const candidates = [
      { detection_id: "d1", kind: "dog" as const, box: [0.1, 0.1, 0.2, 0.2] as [number, number, number, number] },
      { detection_id: "d2", kind: "dog" as const, box: [0.6, 0.5, 0.3, 0.3] as [number, number, number, number] },
    ];
    vi.spyOn(api, "createPet").mockResolvedValue(
      added({ assigned: 1, ambiguous: [{ asset_id: "a2", preview_url: "https://s3.test/p2", candidates }] }),
    );
    const add = vi.spyOn(api, "addToPet").mockResolvedValue(added({ assigned: 1 }));
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1", "a2"]} onClose={vi.fn()} onDone={done} />);
    await user.type(await screen.findByLabelText("Name"), "Pet A");
    await user.click(screen.getByRole("button", { name: "Save" }));

    const dialog = await screen.findByRole("dialog", { name: "Which one is Pet A?" });
    expect(within(dialog).getByText(/This photo has 2 animals/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Add" })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: "Animal 2 of 2, dog" }));
    expect(within(dialog).getByRole("button", { name: "Animal 2 of 2, dog" })).toHaveAttribute("aria-pressed", "true");
    expect(within(dialog).getByRole("button", { name: "Animal 1 of 2, dog" })).toHaveAttribute("aria-pressed", "false");
    await user.click(within(dialog).getByRole("button", { name: "Add" }));

    await waitFor(() => expect(done).toHaveBeenCalled());
    expect(add).toHaveBeenCalledWith("p1", { detection_ids: ["d2"] });
    expect(done).toHaveBeenCalledWith(expect.objectContaining({ assigned: 2, made: true }));
  });

  it("skipping a photo with several animals adds nothing for it", async () => {
    vi.spyOn(api, "createPet").mockResolvedValue(
      added({ assigned: 1, ambiguous: [{ asset_id: "a2", preview_url: "https://s3.test/p2", candidates: [{ detection_id: "d1", kind: "dog", box: [0, 0, 0.5, 0.5] }, { detection_id: "d2", kind: "dog", box: [0.5, 0.5, 0.5, 0.5] }] }] }),
    );
    const add = vi.spyOn(api, "addToPet");
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1", "a2"]} onClose={vi.fn()} onDone={done} />);
    await user.type(await screen.findByLabelText("Name"), "Pet A");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await user.click(await screen.findByRole("button", { name: "Skip this photo" }));
    await waitFor(() => expect(done).toHaveBeenCalledWith(expect.objectContaining({ assigned: 1 })));
    expect(add).not.toHaveBeenCalled();
  });

  it("explains the photos that were left out also when one of the others had to be chosen", async () => {
    vi.spyOn(api, "createPet").mockResolvedValue(
      added({
        assigned: 1,
        skipped: [{ asset_id: "a3", reason: "no_animal" }],
        ambiguous: [{ asset_id: "a2", preview_url: "https://s3.test/p2", candidates: [{ detection_id: "d1", kind: "dog", box: [0, 0, 0.5, 0.5] }, { detection_id: "d2", kind: "dog", box: [0.5, 0.5, 0.5, 0.5] }] }],
      }),
    );
    vi.spyOn(api, "addToPet").mockResolvedValue(added({ assigned: 1 }));
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1", "a2", "a3"]} onClose={vi.fn()} onDone={done} />);
    await user.type(await screen.findByLabelText("Name"), "Pet A");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await user.click(await screen.findByRole("button", { name: "Animal 2 of 2, dog" }));
    await user.click(screen.getByRole("button", { name: "Add" }));
    const dialog = await screen.findByRole("dialog", { name: "Made Pet A" });
    expect(within(dialog).getByText("2 photos added.")).toBeInTheDocument();
    expect(within(dialog).getByText("1 photo has no dog or cat that was found")).toBeInTheDocument();
    expect(done).not.toHaveBeenCalled();
  });

  it("tells in plain words which photos could not join, and finishes on Done", async () => {
    vi.spyOn(api, "createPet").mockResolvedValue(
      added({
        assigned: 1,
        skipped: [
          { asset_id: "a2", reason: "no_animal" },
          { asset_id: "a3", reason: "not_scanned" },
          { asset_id: "a4", reason: "in_other_pet" },
        ],
      }),
    );
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a1", "a2", "a3", "a4"]} onClose={vi.fn()} onDone={done} />);
    await user.type(await screen.findByLabelText("Name"), "Pet A");
    await user.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog", { name: "Made Pet A" });
    expect(within(dialog).getByText("1 photo added.")).toBeInTheDocument();
    expect(within(dialog).getByText("1 photo has no dog or cat that was found")).toBeInTheDocument();
    expect(within(dialog).getByText("1 photo has not been looked through for pets yet")).toBeInTheDocument();
    expect(within(dialog).getByText("1 photo belongs to another pet already")).toBeInTheDocument();
    expect(done).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Done" }));
    expect(done).toHaveBeenCalledWith(expect.objectContaining({ name: "Pet A", assigned: 1 }));
  });

  it("names one animal of a photo: every pet is offered, and the kind starts as what the animal is called but can be changed", async () => {
    vi.spyOn(api, "namedPets").mockResolvedValue({ items: [namedPet("p1", "Pet A"), namedPet("p2", "Pet B", 2, { kind: "cat" })] });
    const create = vi.spyOn(api, "createPet").mockResolvedValue(added({ assigned: 1 }));
    const user = userEvent.setup();
    render(<NamePetSheet detectionIds={["d9"]} kind="cat" onClose={vi.fn()} onDone={vi.fn()} />);
    const dialog = await screen.findByRole("dialog", { name: "Add to pet" });
    expect(within(await within(dialog).findByLabelText("Pet")).getAllByRole("option").map((o) => o.textContent)).toEqual(["A new pet", "Pet A (dog)", "Pet B (cat)"]);
    expect(within(dialog).getByRole("radio", { name: "Cat" })).toBeChecked();
    await user.type(within(dialog).getByLabelText("Name"), "Pet C");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ name: "Pet C", kind: "cat", detection_ids: ["d9"] }));
    // The detector called it a cat, the owner says it is a dog: a dog pet.
    create.mockClear();
    await user.click(within(dialog).getByRole("radio", { name: "Dog" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ name: "Pet C", kind: "dog", detection_ids: ["d9"] }));
  });

  it("asks about a photo whose only animal is called the other kind, with it chosen already, and says it will change", async () => {
    vi.spyOn(api, "namedPets").mockResolvedValue({ items: [namedPet("p1", "Pet A")] });
    const candidates = [{ detection_id: "d7", kind: "cat" as const, box: [0.1, 0.1, 0.4, 0.4] as [number, number, number, number] }];
    vi.spyOn(api, "addToPet").mockResolvedValueOnce(added({ assigned: 0, ambiguous: [{ asset_id: "a2", preview_url: "https://s3.test/p2", candidates }] }));
    const add = vi.mocked(api.addToPet);
    add.mockResolvedValueOnce(added({ assigned: 1 }));
    const done = vi.fn();
    const user = userEvent.setup();
    render(<NamePetSheet assetIds={["a2"]} onClose={vi.fn()} onDone={done} />);
    await user.selectOptions(await screen.findByLabelText("Pet"), "p1");
    await user.click(screen.getByRole("button", { name: "Add photos" }));
    const dialog = await screen.findByRole("dialog", { name: "Which one is Pet A?" });
    expect(within(dialog).getByText(/no dog that was found, but it has this animal/)).toBeInTheDocument();
    expect(within(dialog).getByText(/Pet A is a dog. An animal marked as the other kind becomes a dog/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Animal 1 of 1, cat" })).toHaveAttribute("aria-pressed", "true");
    expect(within(dialog).getByText(/1 Cat/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(add).toHaveBeenLastCalledWith("p1", { detection_ids: ["d7"] }));
    await waitFor(() => expect(done).toHaveBeenCalledWith(expect.objectContaining({ assigned: 1 })));
  });
});

describe("petNamedToast", () => {
  it("words it and offers the pet's page", () => {
    const open = vi.fn();
    const t = petNamedToast({ pet: namedPet("p1", "Pet A"), name: "Pet A", assigned: 3, made: true }, open);
    expect(t.message).toBe("Made Pet A with 3 photos");
    t.action!.onClick();
    expect(open).toHaveBeenCalledWith("p1");
    expect(petNamedToast({ pet: null, name: "Pet A", assigned: 0, made: false }, open).action).toBeUndefined();
  });
});
