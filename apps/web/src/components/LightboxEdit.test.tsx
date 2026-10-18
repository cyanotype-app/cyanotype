// Editing from the lightbox's Info panel: the editors, the refresh after a save, the caption under the photo.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { detail, item, photoDetails } from "../test/fixtures";
import { Lightbox } from "./Lightbox";

vi.mock("../map/PlaceMap", () => ({ default: () => <div data-testid="place-map" /> }));

const items = [1, 2].map((n) => item(n));

beforeEach(async () => {
  await new Promise((r) => setTimeout(r, 20));
  window.history.replaceState(null, "");
  vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
});

describe("editing from the panel", () => {
  it("opens the editors, reads the photo again after a save, and shows the caption under the photo", async () => {
    const user = userEvent.setup();
    const edit = vi.spyOn(api, "editAssets").mockResolvedValue({ updated: 1, unchanged: 0, missing: 0, failed: [] });
    let caption: string | null = null;
    const loadDetail = vi.fn(async (id: string) => ({ ...detail(Number(id.split("-")[1])), caption }));
    const loadDetails = vi.fn(async () => photoDetails({ caption }));
    const onEdited = vi.fn();
    render(
      <MemoryRouter>
        <Lightbox items={items} index={0} onIndexChange={() => {}} onClose={() => {}} loadDetail={loadDetail} loadDetails={loadDetails} onEdited={onEdited} />
      </MemoryRouter>,
    );
    await user.keyboard("i");
    await user.click(await screen.findByRole("button", { name: "Add a caption" }));
    expect(screen.getByRole("dialog", { name: "Edit caption" })).toBeInTheDocument();
    caption = "Dinner by the river";
    const loads = loadDetail.mock.calls.length;
    await user.type(screen.getByLabelText("Caption"), "Dinner by the river");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(edit).toHaveBeenCalledWith(["id-1"], { caption: { text: "Dinner by the river" } });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit caption" })).not.toBeInTheDocument());
    expect(onEdited).toHaveBeenCalledWith(items[0]);
    // The photo's detail and its panel were read again.
    await waitFor(() => expect(loadDetail.mock.calls.length).toBeGreaterThan(loads));
    expect((await screen.findAllByText("Dinner by the river")).length).toBeGreaterThanOrEqual(2); // under the photo and in the panel
    await user.click(screen.getByRole("button", { name: "Adjust date and time" }));
    expect(screen.getByRole("dialog", { name: "Adjust date and time" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Adjust date and time" })).not.toBeInTheDocument();
    // Escape closed the editor only: the panel is still there.
    expect(screen.getByRole("complementary", { name: "Photo details" })).toBeInTheDocument();
  });

  it("offers to add a location when the photo has none, and marks edited values", async () => {
    const user = userEvent.setup();
    const bare = photoDetails({
      location: null,
      edits: { date_edited: true, location_edited: false, original_location: null, original_date: photoDetails().date },
    });
    render(
      <MemoryRouter>
        <Lightbox items={items} index={0} onIndexChange={() => {}} onClose={() => {}} loadDetail={async () => detail(1)} loadDetails={async () => bare} />
      </MemoryRouter>,
    );
    await user.keyboard("i");
    expect(await screen.findByRole("button", { name: "Add location" })).toBeInTheDocument();
    expect(screen.getByText("Edited")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add location" }));
    expect(screen.getByRole("dialog", { name: "Set location" })).toBeInTheDocument();
  });
});
