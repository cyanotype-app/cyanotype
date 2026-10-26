import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { UploadPickerProvider } from "../shell/UploadPicker";
import { SlideshowProvider } from "../slideshow/SlideshowHost";
import { fakeLibrary } from "../test/fakeLibrary";
import { pressBar } from "../test/bar";
import { detail, item } from "../test/fixtures";
import { UploadProvider } from "../upload/UploadProvider";
import { Library } from "./Library";

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
  localStorage.clear();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderLibrary = (props: React.ComponentProps<typeof Library> = {}, path = "/photos") =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <UploadPickerProvider>
            <SlideshowProvider>
              <Library {...props} />
            </SlideshowProvider>
          </UploadPickerProvider>
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

// One day's worth of photos, portrait and landscape.
const photos = Array.from({ length: 12 }, (_, n) =>
  item(n + 1, { width: n % 2 ? 3000 : 4000, height: n % 2 ? 4000 : 3000, sort_time: new Date(Date.UTC(2026, 0, 30, 23 - n)).toISOString() }),
);
const columns = () => (document.querySelector("[data-row=tiles]") as HTMLElement).style.gridTemplateColumns;

describe("the Photos page header", () => {
  it("is titled by the view, with Select and one view menu instead of rows of buttons", async () => {
    fakeLibrary(photos);
    const user = userEvent.setup();
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    expect(screen.getByRole("heading", { level: 1, name: "Photos" })).toBeInTheDocument();
    // No tab pills, no big upload buttons, no separate toggles.
    expect(screen.queryByRole("link", { name: "All photos" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Upload photos" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Date taken" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Slideshow" })).toBeNull();
    const select = screen.getByRole("button", { name: "Select" });
    expect(select).toHaveAttribute("aria-pressed", "false");
    await user.click(select);
    expect(select).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("toolbar", { name: "Selection" })).toBeInTheDocument();
    await user.click(select);
    expect(select).toHaveAttribute("aria-pressed", "false");
    // The upload menu is in the header for phones (the sidebar has it on a desktop).
    expect(screen.getByRole("button", { name: "Upload" })).toBeInTheDocument();
  });

  it("keeps every view setting in one menu: size, sort, stacks and the slideshow", async () => {
    fakeLibrary(photos);
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "View options" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("menuitemradio", { name: "Justified" })).toBeNull(); // the screen size picks the layout
    expect(within(menu).getByRole("menuitem", { name: "Larger photos" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Smaller photos" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitemradio", { name: "Date taken" })).toHaveAttribute("aria-checked", "true");
    expect(within(menu).getByRole("menuitemradio", { name: "Date added" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitemradio", { name: "Stacked" })).toHaveAttribute("aria-checked", "true");
    expect(within(menu).getByRole("menuitemradio", { name: "Every photo" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Slideshow" })).toBeInTheDocument();
  });

  it("the other views are titled too, and only offer the settings that apply to them", async () => {
    fakeLibrary(photos.map((p) => ({ ...p, favorite: true })));
    const user = userEvent.setup();
    renderLibrary({ favorites: true }, "/favorites");
    expect(await screen.findByRole("heading", { level: 1, name: "Favorites" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "View options" }));
    expect(await screen.findByRole("menuitem", { name: "Larger photos" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitemradio", { name: "Date added" })).toBeNull(); // the sort is the library's
    expect(screen.queryByRole("menuitemradio", { name: "Every photo" })).toBeNull();
  });
});

describe("grid layout", () => {
  it("is square crops on a phone", async () => {
    fakeLibrary(photos);
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    expect(columns()).toMatch(/^repeat\(\d+, minmax\(0, 1fr\)\)$/); // equal columns
  });

  it("is rows that keep each photo's shape on a desktop", async () => {
    vi.stubGlobal("matchMedia", (q: string) => ({
      matches: q === "(min-width: 48rem)",
      addEventListener: () => {},
      removeEventListener: () => {},
    }));
    fakeLibrary(photos);
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    await waitFor(() => expect(columns()).not.toMatch(/^repeat/));
    expect(columns()).toContain("fr"); // fills the width
  });

  it("changes the size of the tiles from the menu and remembers it in the browser", async () => {
    fakeLibrary(photos);
    const user = userEvent.setup();
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    const cols = () => Number(/repeat\((\d+)/.exec(columns())![1]);
    const before = cols();
    await user.click(screen.getByRole("button", { name: "View options" }));
    await user.click(await screen.findByRole("menuitem", { name: "Smaller photos" }));
    await waitFor(() => expect(cols()).toBe(before + 1));
    expect(JSON.parse(localStorage.getItem("cyanotype.grid-density")!)).toMatchObject({ square: 1 });
    await user.click(screen.getByRole("button", { name: "View options" }));
    await user.click(await screen.findByRole("menuitem", { name: "Larger photos" }));
    await waitFor(() => expect(cols()).toBe(before));
  });
});

describe("empty states", () => {
  it.each([
    ["favorites", "No favorites yet", /heart/],
    ["archive", "The archive is empty", /albums and search/],
    ["recent", "Nothing added yet", /newest first/],
  ] as const)("%s: a title, a hint and the way on", async (view, title, hint) => {
    fakeLibrary([]);
    renderLibrary({ [view]: true }, `/${view}`);
    expect(await screen.findByRole("heading", { name: title })).toBeInTheDocument();
    expect(screen.getByText(hint)).toBeInTheDocument();
    if (view === "recent") expect(screen.getByRole("button", { name: "Upload photos" })).toBeInTheDocument();
    else expect(screen.getByRole("link", { name: "Browse photos" })).toHaveAttribute("href", "/photos");
  });

  it("an empty library offers to upload, and the page still takes drops", async () => {
    fakeLibrary([]);
    renderLibrary();
    expect(await screen.findByRole("heading", { name: "Your library is empty" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Upload photos" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Upload folder" })).toBeInTheDocument();
  });
});

describe("dropping files", () => {
  it("shows the overlay while files are dragged over, and hands the files to the app's one picker", async () => {
    fakeLibrary(photos);
    const lib = renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    const page = lib.container.querySelector("[data-library]")!;
    const files = { types: ["Files"], files: [new File(["x"], "notes.txt", { type: "text/plain" })] };
    fireEvent.dragEnter(page, { dataTransfer: files });
    expect(document.querySelector("[data-drop-overlay]")).not.toBeNull();
    fireEvent.drop(page, { dataTransfer: files });
    // An unsupported type is counted and said, not uploaded.
    expect(await screen.findByText(/1 file was skipped/)).toBeInTheDocument();
    await waitFor(() => expect(document.querySelector("[data-drop-overlay]")).toBeNull());
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/1 file was skipped/)).toBeNull();
  });

  it("ignores a drag that is not carrying files", async () => {
    fakeLibrary(photos);
    const lib = renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    fireEvent.dragEnter(lib.container.querySelector("[data-library]")!, { dataTransfer: { types: ["text/plain"], files: [] } });
    expect(document.querySelector("[data-drop-overlay]")).toBeNull();
  });
});

describe("selecting", () => {
  it("long-press then drag selects a run of photos; tapping toggles one", async () => {
    fakeLibrary(photos);
    vi.useFakeTimers({ shouldAdvanceTime: true });
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    const tile = (n: number) => screen.getByRole("button", { name: new RegExp(`(Open|Select) photo-${n}.jpg`) });
    fireEvent.pointerDown(tile(2), { pointerType: "touch", clientX: 5, clientY: 5 });
    await vi.advanceTimersByTimeAsync(600);
    const target = tile(5);
    document.elementFromPoint = () => target;
    const move = new Event("touchmove", { cancelable: true });
    Object.defineProperty(move, "touches", { value: [{ clientX: 50, clientY: 5 }] });
    document.dispatchEvent(move);
    document.dispatchEvent(new Event("touchend"));
    await waitFor(() => expect(screen.getByRole("toolbar", { name: "Selection" })).toHaveTextContent("4 selected"));
    for (const n of [2, 3, 4, 5]) expect(tile(n)).toHaveAttribute("aria-pressed", "true");
    expect(tile(1)).toHaveAttribute("aria-pressed", "false");
    vi.useRealTimers();
    await userEvent.click(tile(3));
    expect(tile(3)).toHaveAttribute("aria-pressed", "false");
    Reflect.deleteProperty(document, "elementFromPoint");
  });

  it("offers every action: on the bar or in its menu", async () => {
    fakeLibrary(photos);
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    const bar = screen.getByRole("toolbar", { name: "Selection" });
    const names = within(bar).getAllByRole("button").map((b) => b.getAttribute("aria-label") ?? b.textContent);
    await user.click(within(bar).getByRole("button", { name: "More actions" }));
    const more = (await screen.findAllByRole("menuitem")).map((i) => i.textContent);
    const all = [...names, ...more].join("|");
    for (const label of ["Favorite", "Add to album", "Share", "Download", "Tag", "Rate", "Stack", "Archive", "Hide", "Edit", "Move to trash"]) {
      expect(all, label).toContain(label);
    }
    await user.keyboard("{Escape}");
    expect(screen.getByRole("toolbar", { name: "Selection" })).toBeInTheDocument(); // Escape closed the menu, not the selection
    await pressBar(user, "Rate");
    expect(await screen.findByRole("dialog", { name: "Rate photos" })).toBeInTheDocument();
  });
});
