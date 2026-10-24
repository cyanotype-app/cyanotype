// Rotate and crop from the viewer: the keys that open the editor, saving and waiting for the render, an edited photo's own
// picture to work on, "Download edited", and the Info panel's Picture row.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, type AssetDetail, type AssetListItem } from "../api";
import { detail, item, photoDetails } from "../test/fixtures";
import { Lightbox } from "./Lightbox";

vi.mock("../map/PlaceMap", () => ({ default: () => <div data-testid="place-map" /> }));

beforeEach(async () => {
  await new Promise((r) => setTimeout(r, 20));
  window.history.replaceState(null, "");
  vi.spyOn(api, "tiles").mockResolvedValue({ world: false, detail: false });
});

function mount(opts: { items?: AssetListItem[]; loadDetail?: (id: string) => Promise<AssetDetail>; onEdited?: (i: AssetListItem) => void; onClose?: () => void } = {}) {
  const items = opts.items ?? [item(1), item(2)];
  const loadDetail = opts.loadDetail ?? vi.fn(async (id: string) => detail(Number(id.split("-")[1])));
  const onClose = opts.onClose ?? vi.fn();
  const onEdited = opts.onEdited ?? vi.fn();
  render(
    <MemoryRouter>
      <Lightbox
        items={items}
        index={0}
        onIndexChange={() => {}}
        onClose={onClose}
        loadDetail={loadDetail}
        loadDetails={async () => photoDetails()}
        onEdited={onEdited}
        pollMs={10}
      />
    </MemoryRouter>,
  );
  return { loadDetail, onClose, onEdited };
}

const edit = () => vi.spyOn(api, "editAssets").mockResolvedValue({ updated: 1, unchanged: 0, missing: 0, failed: [] });
const cropButton = () => screen.getByRole("button", { name: "Rotate and crop" });

describe("rotate and crop in the viewer", () => {
  it("opens with the Crop button or e, works on the photo's picture, and Escape closes only the editor", async () => {
    const user = userEvent.setup();
    const { onClose } = mount();
    await waitFor(() => expect(cropButton()).toBeEnabled());
    await user.click(cropButton());
    const dialog = screen.getByRole("dialog", { name: "Rotate and crop" });
    expect(dialog.querySelector("img")).toHaveAttribute("src", "https://s3.test/preview-1");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Rotate and crop" })).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("e");
    expect(screen.getByRole("dialog", { name: "Rotate and crop" })).toBeInTheDocument();
  });

  it("r opens it already turned and c opens it cropping; the viewer's own keys stay out of the way", async () => {
    const user = userEvent.setup();
    const rate = vi.fn();
    const { onClose } = mount();
    void rate;
    await waitFor(() => expect(cropButton()).toBeEnabled());
    await user.keyboard("r");
    expect(screen.getByText(/Rotated 90° right/)).toBeInTheDocument();
    // In the editor Escape and the arrows are the editor's, not the viewer's.
    await user.keyboard("{ArrowRight}");
    await user.keyboard("{Escape}");
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("c");
    expect(screen.getAllByRole("button", { name: /^Crop (corner|edge)/ })).toHaveLength(8);
  });

  it("saves the recipe, shows that the picture is being updated, and tells the page when it is ready", async () => {
    const user = userEvent.setup();
    const e = edit();
    // Before the save the photo has no recipe; then it has one and is rendering; then the render is done.
    let phase: "plain" | "rendering" | "done" = "plain";
    const loadDetail = vi.fn(async (id: string) => ({
      ...detail(Number(id.split("-")[1])),
      rendering: phase === "rendering",
      recipe: phase === "plain" ? null : { rotate: 90 },
      edit_base_url: phase === "done" ? "https://s3.test/uncropped" : null,
    }));
    const onEdited = vi.fn();
    mount({ loadDetail, onEdited });
    await waitFor(() => expect(cropButton()).toBeEnabled());
    await user.keyboard("r");
    phase = "rendering"; // from the moment the edit is saved, the worker has a render to do
    await user.keyboard("{Enter}");
    await waitFor(() => expect(e).toHaveBeenCalledWith(["id-1"], { recipe: { rotate: 90, straighten: 0 } }));
    // The editor closes; the old picture stays under a note while the worker renders the new one.
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Rotate and crop" })).not.toBeInTheDocument());
    expect(await screen.findByRole("status")).toHaveTextContent("Updating the picture…");
    expect(cropButton()).toBeDisabled();
    phase = "done";
    await waitFor(() => expect(screen.queryByText("Updating the picture…")).not.toBeInTheDocument());
    // The page is told twice: at once (the size changed) and when the new thumbnail exists.
    await waitFor(() => expect(onEdited).toHaveBeenCalledTimes(2));
  });

  it("keeps the editor open with the reason when the save fails", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "editAssets").mockResolvedValue({ updated: 0, unchanged: 0, missing: 0, failed: ["id-1"] });
    mount();
    await waitFor(() => expect(cropButton()).toBeEnabled());
    await user.keyboard("r");
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save the changes");
    expect(screen.getByRole("dialog", { name: "Rotate and crop" })).toBeInTheDocument();
  });

  it("works on the whole picture of an edited photo, and can go back to the original", async () => {
    const user = userEvent.setup();
    const e = edit();
    const edited = item(1, { edited: true, width: 1500, height: 2000 });
    const loadDetail = vi.fn(async () => ({
      ...detail(1),
      edited: true,
      recipe: { rotate: 90, crop: { x: 0.1, y: 0.1, w: 0.5, h: 0.5 } },
      edit_base_url: "https://s3.test/uncropped-1",
      file_width: 4000,
      file_height: 3000,
    }));
    mount({ items: [edited, item(2)], loadDetail });
    await waitFor(() => expect(cropButton()).toBeEnabled());
    await user.click(cropButton());
    const dialog = screen.getByRole("dialog", { name: "Rotate and crop" });
    expect(dialog.querySelector("img")).toHaveAttribute("src", "https://s3.test/uncropped-1");
    expect(within(dialog).getByText(/Rotated 90° right, cropped to 1500 × 2000/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Revert to original" }));
    await waitFor(() => expect(e).toHaveBeenCalledWith(["id-1"], { recipe: { revert: true } }));
  });

  it("will not open while the worker is still rendering, or before the picture to work on exists", async () => {
    const user = userEvent.setup();
    const loadDetail = vi.fn(async () => ({ ...detail(1), recipe: { rotate: 90 }, rendering: true, edit_base_url: null }));
    mount({ loadDetail });
    expect(await screen.findByRole("status")).toHaveTextContent("Updating the picture…");
    expect(cropButton()).toBeDisabled();
    await user.keyboard("r");
    expect(screen.queryByRole("dialog", { name: "Rotate and crop" })).not.toBeInTheDocument();
  });

  it("describes the picture in the Info panel and opens the editor from it", async () => {
    const user = userEvent.setup();
    mount();
    await waitFor(() => expect(cropButton()).toBeEnabled());
    await user.keyboard("i");
    const panel = await screen.findByRole("complementary", { name: "Photo details" });
    await waitFor(() => expect(within(panel).getByText("Picture")).toBeInTheDocument());
    expect(within(panel).getByText("Not edited")).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "Rotate and crop the picture" }));
    expect(screen.getByRole("dialog", { name: "Rotate and crop" })).toBeInTheDocument();
  });
});

describe("downloading the edited photo", () => {
  const assign = vi.fn();
  beforeEach(() => {
    assign.mockReset();
    Object.defineProperty(window, "location", { configurable: true, value: { ...window.location, assign } });
  });

  it("offers the original and the edited JPEG, asks until the worker has written it, then downloads", async () => {
    const user = userEvent.setup();
    const status = vi.spyOn(api, "editedStatus").mockResolvedValueOnce("pending").mockResolvedValueOnce("pending").mockResolvedValue("ready");
    mount({ items: [item(1, { edited: true }), item(2)] });
    await user.click(screen.getByRole("button", { name: "Download" }));
    const menu = document.getElementById("lightbox-downloads")!;
    expect(within(menu).getByRole("link", { name: /Download original/ })).toHaveAttribute("href", "/assets/id-1/download");
    await user.click(within(menu).getByRole("button", { name: "Download edited (JPEG)" }));
    expect(within(menu).getByRole("button", { name: /Preparing/ })).toBeDisabled();
    await waitFor(() => expect(assign).toHaveBeenCalledWith("/assets/id-1/download/edited"));
    expect(status).toHaveBeenCalledTimes(3);
  });

  it("says so when the edited photo cannot be prepared", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "editedStatus").mockRejectedValue(new Error("this photo has no rotate or crop to apply"));
    mount({ items: [item(1, { edited: true }), item(2)] });
    await user.click(screen.getByRole("button", { name: "Download" }));
    await user.click(screen.getByRole("button", { name: "Download edited (JPEG)" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no rotate or crop");
    expect(assign).not.toHaveBeenCalled();
  });

  it("a photo that is not edited downloads as before", () => {
    mount();
    expect(screen.getByRole("link", { name: "Download" })).toHaveAttribute("href", "/assets/id-1/download");
  });
});
