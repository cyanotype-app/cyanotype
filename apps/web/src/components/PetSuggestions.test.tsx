import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type PetSuggestion } from "../api";
import { namedPet } from "../test/fixtures";
import { PetSuggestions } from "./PetSuggestions";

afterEach(() => vi.restoreAllMocks());

const pet = namedPet("p1", "Pet A", 5, { pending: 5 });
const sug = (n: number, tier: "likely" | "maybe" = "likely"): PetSuggestion => ({
  detection_id: `d${n}`,
  asset_id: `a${n}`,
  box: [0.1, 0.1, 0.3, 0.3],
  preview_url: `https://s3.test/p${n}`,
  kind: "dog",
  similarity: 0.8,
  tier,
});

/** The server's answers for a list of proposals: `tier` picks the tier, `counts` are of all of them. */
function serve(likely: PetSuggestion[], maybe: PetSuggestion[] = []) {
  return vi.spyOn(api, "petSuggestions").mockImplementation(async (_id, opts = {}) => ({
    items: opts.tier === "maybe" ? maybe : opts.tier === "likely" ? likely : [...likely, ...maybe],
    counts: { likely: likely.length, maybe: maybe.length },
  }));
}

const setup = (over: { onAnswered?: () => void; failure?: () => void; show?: () => void } = {}) => {
  const onAnswered = over.onAnswered ?? vi.fn();
  const failure = over.failure ?? vi.fn();
  const show = over.show ?? vi.fn();
  render(<PetSuggestions pet={pet} onAnswered={onAnswered} failure={failure} show={show} />);
  return { onAnswered, failure, show };
};

describe("PetSuggestions", () => {
  it("asks 'Is this X?' with the likely animals first and counts both tiers", async () => {
    const read = serve([sug(1), sug(2)], [sug(3, "maybe")]);
    setup();
    const section = await screen.findByRole("region", { name: "Suggestions" });
    expect(within(section).getByRole("heading", { name: "Is this Pet A?" })).toBeInTheDocument();
    expect(await within(section).findAllByRole("button", { name: "See the whole photo" })).toHaveLength(2);
    expect(within(section).getByRole("radio", { name: "Likely (2)" })).toBeChecked();
    expect(within(section).getByRole("radio", { name: "Maybe (1)" })).toBeInTheDocument();
    expect(read).toHaveBeenCalledWith("p1", { tier: "likely", limit: 60 });
  });

  it("says when an animal is marked as the other kind, and that a yes to all makes it the pet's kind", async () => {
    serve([sug(1), { ...sug(2), kind: "cat" }]);
    setup();
    const section = await screen.findByRole("region", { name: "Suggestions" });
    expect(await within(section).findAllByText("Marked as a cat")).toHaveLength(1);
    await userEvent.click(within(section).getByRole("button", { name: "Yes to these 2" }));
    expect(await screen.findByText(/Any marked as a cat become a dog/)).toBeInTheDocument();
  });

  it("shows the maybe animals when nothing is likely, and is not there when there is nothing", async () => {
    serve([], [sug(3, "maybe")]);
    const first = render(<PetSuggestions pet={pet} onAnswered={vi.fn()} failure={vi.fn()} show={vi.fn()} />);
    expect(await screen.findAllByRole("button", { name: "See the whole photo" })).toHaveLength(1);
    expect(screen.queryByRole("radiogroup", { name: "Show" })).not.toBeInTheDocument(); // one tier: nothing to switch
    first.unmount();
    serve([], []);
    const empty = render(<PetSuggestions pet={pet} onAnswered={vi.fn()} failure={vi.fn()} show={vi.fn()} />);
    await waitFor(() => expect(api.petSuggestions).toHaveBeenCalled());
    expect(empty.container).toBeEmptyDOMElement();
  });

  it("switches to the maybe tier", async () => {
    const read = serve([sug(1)], [sug(3, "maybe"), sug(4, "maybe")]);
    const user = userEvent.setup();
    setup();
    await screen.findByRole("radio", { name: "Maybe (2)" });
    await user.click(screen.getByRole("radio", { name: "Maybe (2)" }));
    await waitFor(() => expect(screen.getAllByRole("button", { name: "See the whole photo" })).toHaveLength(2));
    expect(read).toHaveBeenLastCalledWith("p1", { tier: "maybe", limit: 60 });
  });

  it("answers Yes: the tile goes at once, the server is told, and the proposals are read again", async () => {
    const read = serve([sug(1), sug(2)]);
    const answer = vi.spyOn(api, "answerPetSuggestions").mockResolvedValue({ confirmed: 1, rejected: 0, skipped: 0 });
    const user = userEvent.setup();
    const { onAnswered, show } = setup();
    await screen.findAllByRole("button", { name: "Yes, this is Pet A" });
    read.mockClear();
    // After a Yes the animal is no longer proposed.
    serve([sug(2)]);
    await user.click(screen.getAllByRole("button", { name: "Yes, this is Pet A" })[0]!);
    expect(answer).toHaveBeenCalledWith("p1", { confirm: ["d1"] });
    await waitFor(() => expect(screen.getAllByRole("button", { name: "See the whole photo" })).toHaveLength(1));
    expect(show).toHaveBeenCalledWith({ message: "Added to Pet A" });
    expect(onAnswered).toHaveBeenCalledWith(1);
  });

  it("answers No by refusing the animal for the pet", async () => {
    serve([sug(1), sug(2), sug(3), sug(4), sug(5), sug(6), sug(7), sug(8)]);
    const answer = vi.spyOn(api, "answerPetSuggestions").mockResolvedValue({ confirmed: 0, rejected: 1, skipped: 0 });
    const user = userEvent.setup();
    const { onAnswered } = setup();
    await user.click((await screen.findAllByRole("button", { name: "No, this is not Pet A" }))[2]!);
    expect(answer).toHaveBeenCalledWith("p1", { reject: ["d3"] });
    await waitFor(() => expect(screen.getAllByRole("button", { name: "See the whole photo" })).toHaveLength(7));
    expect(onAnswered).toHaveBeenCalledWith(0);
  });

  it("brings the tile back and says so when the server refuses", async () => {
    serve([sug(1), sug(2)]);
    vi.spyOn(api, "answerPetSuggestions").mockRejectedValue(new ApiError(500, "boom"));
    const user = userEvent.setup();
    const { failure } = setup();
    await user.click((await screen.findAllByRole("button", { name: "Yes, this is Pet A" }))[0]!);
    await waitFor(() => expect(failure).toHaveBeenCalledWith(expect.any(ApiError), "Couldn’t save your answer."));
    expect(screen.getAllByRole("button", { name: "See the whole photo" })).toHaveLength(2);
  });

  it("confirms every shown animal at once, after asking", async () => {
    serve([sug(1), sug(2), sug(3)]);
    const answer = vi.spyOn(api, "answerPetSuggestions").mockResolvedValue({ confirmed: 3, rejected: 0, skipped: 0 });
    const user = userEvent.setup();
    const { show } = setup();
    await user.click(await screen.findByRole("button", { name: "Yes to these 3" }));
    const sheet = await screen.findByRole("dialog", { name: "Add 3 to Pet A?" });
    expect(answer).not.toHaveBeenCalled();
    await user.click(within(sheet).getByRole("button", { name: "Yes to 3" }));
    await waitFor(() => expect(answer).toHaveBeenCalledWith("p1", { confirm: ["d1", "d2", "d3"] }));
    expect(show).toHaveBeenCalledWith({ message: "Added 3 photos to Pet A" });
  });

  it("refuses every shown animal at once, after a warning that it is for good", async () => {
    serve([sug(1), sug(2)]);
    const answer = vi.spyOn(api, "answerPetSuggestions").mockResolvedValue({ confirmed: 0, rejected: 2, skipped: 0 });
    const user = userEvent.setup();
    setup();
    await user.click(await screen.findByRole("button", { name: "No to these 2" }));
    const sheet = await screen.findByRole("dialog", { name: "None of these 2 are Pet A?" });
    expect(within(sheet).getByText("They will not be suggested for Pet A again.")).toBeInTheDocument();
    await user.click(within(sheet).getByRole("button", { name: "No to 2" }));
    await waitFor(() => expect(answer).toHaveBeenCalledWith("p1", { reject: ["d1", "d2"] }));
  });

  it("shows the whole photo with the animal outlined, and answers from there", async () => {
    serve([sug(1)]);
    const answer = vi.spyOn(api, "answerPetSuggestions").mockResolvedValue({ confirmed: 1, rejected: 0, skipped: 0 });
    const user = userEvent.setup();
    setup();
    await user.click(await screen.findByRole("button", { name: "See the whole photo" }));
    const dialog = await screen.findByRole("dialog", { name: "Is this Pet A?" });
    expect(within(dialog).getByAltText("The photo, with the animal in question outlined")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Not a face" })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Yes, this is Pet A" }));
    await waitFor(() => expect(answer).toHaveBeenCalledWith("p1", { confirm: ["d1"] }));
  });
});
