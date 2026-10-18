import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type EditResult } from "../api";
import { photoDetails } from "../test/fixtures";
import { EditDialog, type EditKind } from "./EditDialog";

// WebGL is not available: the picker is a stand-in that "picks" a point when clicked.
vi.mock("../map/LocationPicker", () => ({
  default: (p: { point: { latitude: number; longitude: number } | null; onPick: (x: { latitude: number; longitude: number }) => void }) => (
    <button type="button" data-testid="picker" onClick={() => p.onPick({ latitude: 10.5, longitude: 20.25 })}>
      {p.point ? `${p.point.latitude},${p.point.longitude}` : "no pin"}
    </button>
  ),
}));

const ok: EditResult = { updated: 1, unchanged: 0, missing: 0, failed: [] };
const d = photoDetails();
const single = {
  date: d.date,
  caption: null,
  edits: { date_edited: false, location_edited: false, original_date: null, original_location: null },
  location: { latitude: 38.7223, longitude: -9.1393 },
};

function setup(kind: EditKind, count = 1, current: Parameters<typeof EditDialog>[0]["current"] = count === 1 ? single : undefined) {
  const onDone = vi.fn();
  const onClose = vi.fn();
  const ids = Array.from({ length: count }, (_, i) => `id-${i + 1}`);
  render(<EditDialog kind={kind} target={ids} count={count} current={current} onClose={onClose} onDone={onDone} />);
  return { onDone, onClose, ids };
}

beforeEach(() => {
  vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
  vi.spyOn(api, "places").mockResolvedValue([
    { city: "Lisbon", region: "Lisbon", country: "Portugal", country_code: "PT", latitude: 38.72, longitude: -9.14, count: 12 },
  ]);
});

describe("EditDialog", () => {
  it("sets one photo's date and time with its zone, starting from what it has", async () => {
    const user = userEvent.setup();
    const edit = vi.spyOn(api, "editAssets").mockResolvedValue(ok);
    const { onDone } = setup("date");
    const input = screen.getByLabelText("Date and time");
    expect(input).toHaveValue("2023-07-04T13:05:09");
    expect(screen.getByLabelText("Time zone")).toHaveValue("+02:00");
    await user.clear(input);
    await user.type(input, "2022-01-02T03:04:05");
    await user.selectOptions(screen.getByLabelText("Time zone"), "-05:00");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(edit).toHaveBeenCalledWith(["id-1"], { date: { local: "2022-01-02T03:04:05", offset: "-05:00" } });
    await waitFor(() => expect(onDone).toHaveBeenCalledWith(ok));
  });

  it("sends an unknown zone as null", async () => {
    const user = userEvent.setup();
    const edit = vi.spyOn(api, "editAssets").mockResolvedValue(ok);
    setup("date");
    await user.selectOptions(screen.getByLabelText("Time zone"), "");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(edit).toHaveBeenCalledWith(["id-1"], { date: { local: "2023-07-04T13:05:09", offset: null } });
  });

  it("shifts several photos by an offset, and will not save a shift of nothing", async () => {
    const user = userEvent.setup();
    const edit = vi.spyOn(api, "editAssets").mockResolvedValue({ ...ok, updated: 3 });
    setup("date", 3);
    expect(screen.getByRole("dialog", { name: "Adjust date and time, 3 photos" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.selectOptions(screen.getByLabelText("Direction"), "earlier");
    await user.clear(screen.getByLabelText("Hours"));
    await user.type(screen.getByLabelText("Hours"), "2");
    await user.type(screen.getByLabelText("Days"), "1");
    expect(screen.getByRole("status")).toHaveTextContent("1 day 2 hours earlier");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(edit).toHaveBeenCalledWith(["id-1", "id-2", "id-3"], { date: { shift_seconds: -(86_400 + 7_200) } });
  });

  it("reverts to the file's values and says what they are", async () => {
    const user = userEvent.setup();
    const edit = vi.spyOn(api, "editAssets").mockResolvedValue(ok);
    setup("date", 1, {
      ...single,
      edits: { date_edited: true, location_edited: false, original_location: null, original_date: { ...d.date, local: "2020-05-06T07:08:09", offset: "+01:00" } },
    });
    expect(screen.getByText(/The file says 2020-05-06 07:08:09 UTC\+01:00/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Revert to original" }));
    expect(edit).toHaveBeenCalledWith(["id-1"], { date: { revert: true } });
  });

  it("sets a location from the map, from your places, or from typed coordinates", async () => {
    const user = userEvent.setup();
    const edit = vi.spyOn(api, "editAssets").mockResolvedValue(ok);
    setup("location", 2);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.click(await screen.findByTestId("picker"));
    expect(screen.getByLabelText("Coordinates")).toHaveValue("10.50000, 20.25000");
    await user.click(await screen.findByRole("button", { name: /Lisbon, Portugal/ }));
    expect(screen.getByLabelText("Coordinates")).toHaveValue("38.72000, -9.14000");
    await user.clear(screen.getByLabelText("Coordinates"));
    await user.type(screen.getByLabelText("Coordinates"), "48.8566, 2.3522");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(edit).toHaveBeenCalledWith(["id-1", "id-2"], { location: { latitude: 48.8566, longitude: 2.3522 } });
  });

  it("searches only your own places", async () => {
    const user = userEvent.setup();
    setup("location", 1, { ...single, location: null });
    await user.type(screen.getByLabelText("Search your places"), "lis");
    await waitFor(() => expect(api.places).toHaveBeenLastCalledWith("lis", expect.anything()));
  });

  it("saves, clears and removes a caption", async () => {
    const user = userEvent.setup();
    const edit = vi.spyOn(api, "editAssets").mockResolvedValue(ok);
    setup("caption", 1, { ...single, caption: "old words" });
    const box = screen.getByLabelText("Caption");
    expect(box).toHaveValue("old words");
    await user.clear(box);
    await user.type(box, "Dinner by the river");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(edit).toHaveBeenLastCalledWith(["id-1"], { caption: { text: "Dinner by the river" } });
    await user.click(screen.getByRole("button", { name: "Remove caption" }));
    expect(edit).toHaveBeenLastCalledWith(["id-1"], { caption: { revert: true } });
  });

  it("keeps the dialog open on an error, and retries only the photos that failed", async () => {
    const user = userEvent.setup();
    const edit = vi
      .spyOn(api, "editAssets")
      .mockResolvedValueOnce({ updated: 1, unchanged: 0, missing: 0, failed: ["id-2"] })
      .mockRejectedValueOnce(new ApiError(400, "a caption is at most 2000 characters"))
      .mockResolvedValueOnce(ok);
    const { onDone } = setup("caption", 2);
    await user.type(screen.getByLabelText("Caption for all 2 photos"), "hello");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("1 of 2 could not be saved");
    expect(onDone).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("a caption is at most 2000 characters");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(edit.mock.calls.map((c) => c[0])).toEqual([["id-1", "id-2"], ["id-2"], ["id-2"]]);
  });

  it("closes on Escape and Cancel", async () => {
    const user = userEvent.setup();
    const { onClose } = setup("caption");
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
