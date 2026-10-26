import { configure, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type AssetListItem, type StackDetail } from "../api";
import { AuthProvider } from "../auth";
import { UploadProvider } from "../upload/UploadProvider";
import { UploadPickerProvider } from "../shell/UploadPicker";
import { barDisabled, chooseView, pressBar } from "../test/bar";
import { detail, item } from "../test/fixtures";
import { fakeLibrary } from "../test/fakeLibrary";
import { Library } from "./Library";

// Photos 1 and 5 are on their own; photo 2 stands for a stack of 2, 3 and 4 (the cover is 2).
const stackTile = (): AssetListItem => ({ ...item(2), stack: { size: 3, members: ["id-2", "id-3", "id-4"] } });
const stackDetail = (over: Partial<StackDetail> = {}): StackDetail => ({
  cover_id: "id-2",
  manual: false,
  items: [item(2), item(3), item(4)],
  ...over,
});

// The page loads its prefs, the counts and a page of photos before a tile exists: allow for a busy machine.
configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 20_000 });

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
  vi.spyOn(api, "getDetails").mockRejectedValue(new Error("not needed"));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderLibrary = () =>
  render(
    <MemoryRouter initialEntries={["/library"]}>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <UploadPickerProvider>
            <Library />
          </UploadPickerProvider>
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

const open = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(await screen.findByRole("button", { name: "Open photo-2.jpg, stack of 3 photos" }));
  return screen.findByRole("dialog");
};

describe("stacks in the library", () => {
  it("shows a stack as one tile with its size and the toggle that lists every photo", async () => {
    const lib = fakeLibrary([item(1), stackTile(), item(5)]);
    const setPrefs = vi.spyOn(api, "setPrefs").mockResolvedValue({ memories_enabled: true, library_sort: "taken", stacks_collapsed: false });
    const user = userEvent.setup();
    renderLibrary();
    const tile = await screen.findByRole("button", { name: "Open photo-2.jpg, stack of 3 photos" });
    expect(within(tile).getByText("3")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open photo-1.jpg" })).toBeInTheDocument();
    expect(lib.listAssets).toHaveBeenCalledWith(expect.objectContaining({ stacks: "collapsed" }));

    await user.click(screen.getByRole("button", { name: "View options" }));
    expect(await screen.findByRole("menuitemradio", { name: "Stacked" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{Escape}");
    await chooseView(user, "Every photo");
    expect(setPrefs).toHaveBeenCalledWith({ stacks_collapsed: false });
    await waitFor(() => expect(lib.listAssets).toHaveBeenCalledWith(expect.objectContaining({ stacks: "expanded" })));
  });

  it("steps through the stack in the viewer with the strip and the keys, and the arrows still move through the timeline", async () => {
    fakeLibrary([item(1), stackTile(), item(5)]);
    const getStack = vi.spyOn(api, "getStack").mockResolvedValue(stackDetail());
    const user = userEvent.setup();
    renderLibrary();
    const dialog = await open(user);
    expect(getStack).toHaveBeenCalledWith("id-2");
    expect(await within(dialog).findByText("Photo 1 of 3 in this stack")).toBeInTheDocument();

    await user.keyboard("]");
    expect(await within(dialog).findByText("Photo 2 of 3 in this stack")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "photo-4.jpg" }));
    expect(within(dialog).getByText("Photo 3 of 3 in this stack")).toBeInTheDocument();
    await user.keyboard("]");
    expect(within(dialog).getByText("Photo 3 of 3 in this stack")).toBeInTheDocument();
    await user.keyboard("[[");
    expect(within(dialog).getByText("Photo 2 of 3 in this stack")).toBeInTheDocument();

    // ArrowRight goes to the next photo of the timeline (photo 5), which is not in a stack: the strip goes.
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(within(dialog).queryByRole("region", { name: "Stack" })).not.toBeInTheDocument());
    expect(screen.getByRole("dialog", { name: "photo-5.jpg" })).toBeInTheDocument();
  });

  it("sets the photo on show as the cover, and keeps the viewer where it is", async () => {
    const lib = fakeLibrary([item(1), stackTile(), item(5)]);
    vi.spyOn(api, "getStack").mockResolvedValue(stackDetail());
    const cover = vi.spyOn(api, "setStackCover").mockResolvedValue({ cover_id: "id-3" });
    const user = userEvent.setup();
    renderLibrary();
    const dialog = await open(user);
    await user.keyboard("]");
    await within(dialog).findByText("Photo 2 of 3 in this stack");
    expect(within(dialog).getByRole("button", { name: "Set as stack cover" })).toBeEnabled();
    const reads = lib.timeline.mock.calls.length;
    await user.click(within(dialog).getByRole("button", { name: "Set as stack cover" }));
    await waitFor(() => expect(cover).toHaveBeenCalledWith("id-3"));
    expect(await screen.findByText("Cover changed")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(lib.timeline.mock.calls.length).toBe(reads);
    // The timeline is read again once the viewer closes (the tile may have moved).
    await user.keyboard("{Escape}");
    await waitFor(() => expect(lib.timeline.mock.calls.length).toBeGreaterThan(reads));
  });

  it("takes a photo out of its stack, or dissolves the stack, and closes the viewer", async () => {
    const lib = fakeLibrary([item(1), stackTile(), item(5)]);
    vi.spyOn(api, "getStack").mockResolvedValue(stackDetail());
    const remove = vi.spyOn(api, "removeFromStack").mockResolvedValue({ removed: "id-4" });
    const unstack = vi.spyOn(api, "unstack").mockResolvedValue({ count: 3 });
    const stackAgain = vi.spyOn(api, "stackAssets").mockResolvedValue({ size: 3, cover_id: "id-2", previous: [] });
    const user = userEvent.setup();
    renderLibrary();
    let dialog = await open(user);
    await user.click(await within(dialog).findByRole("button", { name: "photo-4.jpg" }));
    const reads = lib.timeline.mock.calls.length;
    await user.click(within(dialog).getByRole("button", { name: "Remove from stack" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("id-4"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(lib.timeline.mock.calls.length).toBeGreaterThan(reads));

    dialog = await open(user);
    await user.click(await within(dialog).findByRole("button", { name: "Unstack all" }));
    await waitFor(() => expect(unstack).toHaveBeenCalledWith("id-2"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    // Undo stacks the same photos again, the cover first.
    await user.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(stackAgain).toHaveBeenCalledWith(["id-2", "id-3", "id-4"]));
  });

  it("stacks the selected tiles by hand, sending every photo a collapsed stack stands for", async () => {
    fakeLibrary([item(1), stackTile(), item(5)]);
    const previous = [
      { id: "id-1", stack_id: null, stack_cover: false },
      { id: "id-2", stack_id: "burst-x", stack_cover: true },
    ];
    const stack = vi.spyOn(api, "stackAssets").mockResolvedValue({ size: 5, cover_id: "id-1", previous });
    const restore = vi.spyOn(api, "restoreStacking").mockResolvedValue({ count: 2 });
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Select" }));
    const bar = screen.getByRole("toolbar", { name: "Selection" });
    expect(await barDisabled(user, "Stack")).toBe(true);
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    expect(await barDisabled(user, "Stack")).toBe(true);
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Select photo-2.jpg and 2 more from its stack" }));
    expect(within(bar).getByText("plus 2 more in stacks")).toBeInTheDocument();
    await pressBar(user, "Stack");
    await waitFor(() => expect(stack).toHaveBeenCalledWith(["id-1", "id-2", "id-3", "id-4"]));
    expect(await screen.findByText("5 photos stacked")).toBeInTheDocument();
    // Undo hands back what the server said the photos were before, so merged stacks return too.
    await user.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(restore).toHaveBeenCalledWith(previous));
  });

  it("trashes every photo a selected stack tile stands for", async () => {
    fakeLibrary([item(1), stackTile(), item(5)]);
    const trash = vi.spyOn(api, "trashAssets").mockResolvedValue({ count: 3 });
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-2.jpg and 2 more from its stack" }));
    await user.click(screen.getByRole("button", { name: "Move to trash" }));
    await waitFor(() => expect(trash).toHaveBeenCalledWith(["id-2", "id-3", "id-4"]));
  });
});
