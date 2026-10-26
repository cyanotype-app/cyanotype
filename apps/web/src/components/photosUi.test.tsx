import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { item } from "../test/fixtures";
import { tileElement, tileRect } from "../ui/sharedTile";
import "../test/dialogs";
import { SelectionBar, type BarAction } from "./SelectionBar";
import { Tile } from "./Tile";
import { useToast } from "./Toast";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const noop = () => undefined;

describe("Tile", () => {
  it("marks its picture with the photo's id for the viewer, and fades it in once it has loaded", () => {
    render(<Tile item={item(1)} index={0} onOpen={noop} onThumbError={noop} selecting={false} selected={false} />);
    const img = screen.getByAltText("photo-1.jpg");
    expect(img).toHaveAttribute("data-asset-id", "id-1");
    expect(img.className).toContain("opacity-0");
    fireEvent.load(img);
    expect(img.className).toContain("opacity-100");
    expect(tileElement("id-1")).toBe(img);
  });

  it("marks the placeholder of a photo that has no thumbnail yet", () => {
    render(<Tile item={item(2, { thumb_url: null })} index={0} onOpen={noop} onThumbError={noop} selecting={false} selected={false} />);
    expect(screen.getByText("Processing…").closest("[data-asset-id]")).toHaveAttribute("data-asset-id", "id-2");
  });

  it("shows a check circle while selecting, and says which tiles are chosen", () => {
    const { rerender } = render(<Tile item={item(1)} index={0} onOpen={noop} onThumbError={noop} selecting selected={false} />);
    expect(screen.getByRole("button", { name: "Select photo-1.jpg" })).toHaveAttribute("aria-pressed", "false");
    rerender(<Tile item={item(1)} index={0} onOpen={noop} onThumbError={noop} selecting selected />);
    expect(screen.getByRole("button", { name: "Select photo-1.jpg" })).toHaveAttribute("aria-pressed", "true");
    expect(document.querySelector("[data-tile][data-selected]")).not.toBeNull();
  });

  it("badges videos", () => {
    render(<Tile item={item(3, { content_type: "video/mp4" })} index={0} onOpen={noop} onThumbError={noop} selecting={false} selected={false} />);
    expect(screen.getByText("Video")).toBeInTheDocument();
  });

  it("a long press with a finger selects, and the finger then selects the tiles it crosses without opening anything", () => {
    vi.useFakeTimers();
    const onOpen = vi.fn();
    const onLongPress = vi.fn();
    const onDragOver = vi.fn();
    const onDragEnd = vi.fn();
    render(
      <>
        <Tile item={item(1)} index={0} onOpen={onOpen} onThumbError={noop} selecting={false} selected={false} onLongPress={onLongPress} onDragOver={onDragOver} onDragEnd={onDragEnd} />
        <Tile item={item(2)} index={1} onOpen={onOpen} onThumbError={noop} selecting={false} selected={false} onLongPress={onLongPress} onDragOver={onDragOver} onDragEnd={onDragEnd} />
      </>,
    );
    const first = screen.getByRole("button", { name: "Open photo-1.jpg" });
    const second = screen.getByRole("button", { name: "Open photo-2.jpg" });
    fireEvent.pointerDown(first, { pointerType: "touch", clientX: 10, clientY: 10 });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(onLongPress).toHaveBeenCalledWith("id-1");

    // The finger moves onto the second tile.
    document.elementFromPoint = () => second;
    const move = new Event("touchmove", { cancelable: true, bubbles: true });
    Object.defineProperty(move, "touches", { value: [{ clientX: 200, clientY: 10 }] });
    document.dispatchEvent(move);
    expect(onDragOver).toHaveBeenCalledWith("id-2");
    expect(move.defaultPrevented).toBe(true); // the page does not scroll under the finger

    document.dispatchEvent(new Event("touchend"));
    expect(onDragEnd).toHaveBeenCalledOnce();
    fireEvent.click(first); // the click that ends the press
    expect(onOpen).not.toHaveBeenCalled();
    // Once the finger is up the page scrolls again.
    const later = new Event("touchmove", { cancelable: true });
    Object.defineProperty(later, "touches", { value: [{ clientX: 0, clientY: 0 }] });
    document.dispatchEvent(later);
    expect(later.defaultPrevented).toBe(false);
    Reflect.deleteProperty(document, "elementFromPoint");
  });

  it("a finger that moves before the press is long enough is a scroll, not a selection", () => {
    vi.useFakeTimers();
    const onLongPress = vi.fn();
    render(<Tile item={item(1)} index={0} onOpen={noop} onThumbError={noop} selecting={false} selected={false} onLongPress={onLongPress} />);
    const tile = screen.getByRole("button", { name: "Open photo-1.jpg" });
    fireEvent.pointerDown(tile, { pointerType: "touch", clientX: 10, clientY: 10 });
    fireEvent.pointerMove(tile, { pointerType: "touch", clientX: 10, clientY: 60 });
    act(() => {
      vi.advanceTimersByTime(800);
    });
    expect(onLongPress).not.toHaveBeenCalled();
  });
});

describe("tileRect", () => {
  it("gives where the photo's tile is, or null when it is not on the page", () => {
    render(<Tile item={item(1)} index={0} onOpen={noop} onThumbError={noop} selecting={false} selected={false} />);
    expect(tileRect("id-9")).toBeNull();
    // jsdom has no layout: every rect is empty, which is "not on screen" for the viewer.
    expect(tileRect("id-1")).toBeNull();
    const img = screen.getByAltText("photo-1.jpg");
    vi.spyOn(img, "getBoundingClientRect").mockReturnValue(new DOMRect(10, 20, 120, 120));
    expect(tileRect("id-1")).toMatchObject({ x: 10, y: 20, width: 120, height: 120 });
  });
});

describe("SelectionBar", () => {
  const act1 = (id: string, extra: Partial<BarAction> = {}): BarAction => ({ id, label: id, icon: <span />, onSelect: vi.fn(), ...extra });
  const many = () => [act1("Favorite"), act1("Album"), act1("Share"), act1("Download"), act1("Tag"), act1("Rate"), act1("Stack"), act1("Archive"), act1("Trash", { pinned: true, danger: true })];

  it("shows the count and its tools, and the actions that do not fit are in the More menu", async () => {
    const user = userEvent.setup();
    const actions = many();
    render(<SelectionBar count={3} total={50} onSelectAll={noop} onClear={noop} onDone={noop} actions={actions} />);
    const bar = screen.getByRole("toolbar", { name: "Selection" });
    expect(within(bar).getByText("3 selected")).toBeInTheDocument();
    // A phone has five slots: three actions, More and the pinned Trash.
    expect(within(bar).getAllByRole("button").map((b) => b.textContent)).toEqual(
      expect.arrayContaining(["Favorite", "Album", "Share", "More", "Trash"]),
    );
    expect(within(bar).queryByRole("button", { name: "Archive" })).toBeNull();
    await user.click(within(bar).getByRole("button", { name: "More actions" }));
    const items = (await screen.findAllByRole("menuitem")).map((i) => i.textContent);
    expect(items).toEqual(["Download", "Tag", "Rate", "Stack", "Archive"]);
    await user.click(screen.getByRole("menuitem", { name: "Archive" }));
    expect(actions[7]!.onSelect).toHaveBeenCalledOnce();
  });

  it("has no More menu when everything fits, keeps Trash last, and disables what cannot be done", async () => {
    const user = userEvent.setup();
    const trash = act1("Trash", { pinned: true, danger: true });
    render(<SelectionBar count={0} total={0} onSelectAll={noop} onClear={noop} actions={[act1("Favorite", { disabled: true }), trash]} />);
    const bar = screen.getByRole("toolbar", { name: "Selection" });
    expect(within(bar).getByText("Select photos")).toBeInTheDocument();
    expect(within(bar).queryByRole("button", { name: "More actions" })).toBeNull();
    expect(within(bar).getByRole("button", { name: "Favorite" })).toBeDisabled();
    expect(within(bar).getByRole("button", { name: "Select all" })).toBeDisabled();
    expect(within(bar).getByRole("button", { name: "Clear" })).toBeDisabled();
    expect(within(bar).queryByRole("button", { name: "Done" })).toBeNull();
    await user.click(within(bar).getByRole("button", { name: "Trash" }));
    expect(trash.onSelect).toHaveBeenCalledOnce();
  });

  it("still lays out free-form buttons for pages that bring their own", () => {
    render(
      <SelectionBar count={1} total={5} onSelectAll={noop} onClear={noop}>
        <button type="button">Restore</button>
      </SelectionBar>,
    );
    expect(screen.getByRole("button", { name: "Restore" })).toBeInTheDocument();
  });

  it("tells the toasts how high it is, and takes that back when it goes", () => {
    const { unmount } = render(<SelectionBar count={1} total={5} onSelectAll={noop} onClear={noop} actions={[act1("Favorite")]} />);
    expect(document.documentElement.style.getPropertyValue("--selbar-h")).toMatch(/px$/);
    unmount();
    expect(document.documentElement.style.getPropertyValue("--selbar-h")).toBe("");
  });
});

function Toaster({ run }: { run: (t: ReturnType<typeof useToast>) => void }) {
  const t = useToast();
  return (
    <>
      <button type="button" onClick={() => run(t)}>
        go
      </button>
      {t.toast}
    </>
  );
}

describe("Toast", () => {
  it("announces a message, runs its action once and goes", async () => {
    const user = userEvent.setup();
    const undo = vi.fn();
    render(<Toaster run={(t) => t.show({ message: "2 moved to trash", action: { label: "Undo", onClick: undo } })} />);
    await user.click(screen.getByRole("button", { name: "go" }));
    expect(screen.getByRole("status")).toHaveTextContent("2 moved to trash");
    await user.click(screen.getByRole("button", { name: "Undo" }));
    expect(undo).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.queryByText("2 moved to trash")).toBeNull());
  });

  it("is dismissed with the close button, and marks errors", async () => {
    const user = userEvent.setup();
    render(<Toaster run={(t) => t.show({ message: "Couldn’t move to trash.", tone: "error" })} />);
    await user.click(screen.getByRole("button", { name: "go" }));
    expect(screen.getByText("Couldn’t move to trash.").parentElement).toHaveAttribute("data-tone", "error");
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(screen.queryByText("Couldn’t move to trash.")).toBeNull());
  });

  it("goes by itself after its time", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    render(<Toaster run={(t) => t.show({ message: "Saved", ms: 1000 })} />);
    fireEvent.click(screen.getByRole("button", { name: "go" }));
    expect(screen.getByText("Saved")).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    await waitFor(() => expect(screen.queryByText("Saved")).toBeNull());
  });
});
