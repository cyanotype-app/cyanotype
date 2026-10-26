import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, localToday, searchQuery } from "./api";
import { hiddenNote } from "./albumText";
import { AuthProvider } from "./auth";
import { MemoriesStrip } from "./components/MemoriesStrip";
import { Grid } from "./components/Grid";
import { HiddenPage } from "./pages/HiddenPage";
import { Library } from "./pages/Library";
import { chipsOf, paramsFromUrl, urlFromParams } from "./searchText";
import { fakeLibrary } from "./test/fakeLibrary";
import { detail, item } from "./test/fixtures";
import { UploadProvider } from "./upload/UploadProvider";
import { UploadPickerProvider } from "./shell/UploadPicker";
import { chooseView, pressBar } from "./test/bar";
import { archiveMessage, emptyView, hasMemories, memoryCount, msUntil, unlockedUntil, viewTitle, yearsAgo } from "./viewText";

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderIn = (ui: React.ReactNode, path = "/library") =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <UploadPickerProvider>{ui}</UploadPickerProvider>
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

describe("words", () => {
  it("titles the views that are not tabs and says what an empty one is for", () => {
    expect(viewTitle("archive")).toBe("Archive");
    expect(viewTitle("hidden")).toBe("Hidden");
    expect(viewTitle("recent")).toBe("Recently added");
    expect(viewTitle("library")).toBeNull();
    expect(emptyView("library")).toBeNull();
    expect(emptyView("hidden")?.body).toMatch(/password/);
    expect(emptyView("archive")?.title).toBe("The archive is empty");
  });

  it("counts and years", () => {
    expect(archiveMessage(1, true)).toBe("1 archived");
    expect(archiveMessage(1200, false)).toBe("1,200 back in the library");
    expect(yearsAgo(1)).toBe("1 year ago");
    expect(yearsAgo(3)).toBe("3 years ago");
    expect(memoryCount(1)).toBe("1 photo");
    expect(memoryCount(9)).toBe("9 photos");
    expect(hiddenNote(0)).toBe("");
    expect(hiddenNote(undefined)).toBe("");
    expect(hiddenNote(2)).toBe("2 hidden");
  });

  it("no memories, no strip", () => {
    expect(hasMemories([])).toBe(false);
    expect(hasMemories([{ year: 2023, years_ago: 3, count: 0, items: [] }])).toBe(false);
    expect(hasMemories([{ year: 2023, years_ago: 3, count: 1, items: [item(1)] }])).toBe(true);
  });

  it("the unlock clock", () => {
    expect(msUntil("2026-01-01T00:10:00Z", Date.parse("2026-01-01T00:00:00Z"))).toBe(600_000);
    expect(msUntil("2026-01-01T00:00:00Z", Date.parse("2026-01-01T00:05:00Z"))).toBe(0);
    expect(unlockedUntil("2026-01-01T14:05:00Z", "en-US")).toMatch(/^Unlocked until \d{1,2}:05/);
  });

  it("looks up today in the user's own zone", () => {
    expect(localToday(new Date(2026, 9, 9, 23, 30))).toBe("2026-10-09");
  });
});

describe("search: include archived", () => {
  it("is on unless asked otherwise, and only the off state is in a URL, a query or a chip", () => {
    expect(paramsFromUrl(new URLSearchParams("q=cat"))).toEqual({ q: "cat" });
    expect(paramsFromUrl(new URLSearchParams("include_archived=false"))).toEqual({ include_archived: false });
    expect(paramsFromUrl(new URLSearchParams("include_archived=true"))).toEqual({});
    expect(urlFromParams({ q: "cat", include_archived: false })).toBe("q=cat&include_archived=false");
    expect(urlFromParams({ q: "cat", include_archived: true })).toBe("q=cat");
    expect(searchQuery({ q: "cat", include_archived: false })).toContain("include_archived=false");
    expect(searchQuery({ q: "cat", include_archived: true })).toBe("q=cat");
    expect(chipsOf({ include_archived: false }).map((c) => c.label)).toEqual(["Without archived"]);
    expect(chipsOf({ include_archived: true })).toEqual([]);
  });
});

describe("api requests", () => {
  const stubFetch = () => {
    const f = vi.fn(async () => new Response(JSON.stringify({ items: [], next_before: null, total: 0, days: [] })));
    vi.stubGlobal("fetch", f);
    return f;
  };
  const urlOf = (f: ReturnType<typeof stubFetch>) => String((f.mock.calls[0] as unknown[])[0]);

  it("asks for the right view and order", async () => {
    const f = stubFetch();
    await api.listAssets({ archived: true });
    expect(urlOf(f)).toBe("/assets?archived=true");
    f.mockClear();
    await api.listAssets({ hidden: true, limit: 5 });
    expect(urlOf(f)).toBe("/assets?limit=5&hidden=true");
    f.mockClear();
    await api.listAssets({ sort: "added" });
    expect(urlOf(f)).toBe("/assets?sort=added");
    f.mockClear();
    await api.listAssets({ sort: "taken" });
    expect(urlOf(f)).toBe("/assets");
    f.mockClear();
    await api.timeline({ sort: "added" });
    expect(urlOf(f)).toBe("/assets/timeline?sort=added");
    f.mockClear();
    await api.timeline({ favorite: true });
    expect(urlOf(f)).toBe("/assets/timeline?favorite=true");
  });

  it("posts hide, archive, unlock and dismissals", async () => {
    const f = stubFetch();
    await api.hideAssets(["a"], true);
    expect(urlOf(f)).toBe("/assets/hide");
    expect(JSON.parse(String(((f.mock.calls[0] as unknown) as [string, RequestInit])[1].body))).toEqual({ ids: ["a"], hidden: true });
    f.mockClear();
    await api.archiveAssets({ scope: "library", except: ["x"] }, true);
    expect(JSON.parse(String(((f.mock.calls[0] as unknown) as [string, RequestInit])[1].body))).toEqual({
      scope: "library",
      except: ["x"],
      archived: true,
    });
    f.mockClear();
    await api.unlockHidden("pw");
    expect(urlOf(f)).toBe("/auth/hidden/unlock");
    f.mockClear();
    await api.dismissMemories(["a"], false);
    expect(JSON.parse(String(((f.mock.calls[0] as unknown) as [string, RequestInit])[1].body))).toEqual({ ids: ["a"], dismissed: false });
    f.mockClear();
    await api.memories({ all: true, date: "2026-10-19" });
    expect(urlOf(f)).toBe("/assets/memories?date=2026-10-19&all=true");
  });
});

describe("tiles", () => {
  it("flag an archived photo", () => {
    render(<Grid items={[item(1, { archived: true }), item(2)]} onOpen={() => {}} onThumbError={() => {}} />);
    expect(screen.getAllByText("Archived")).toHaveLength(1);
  });
});

describe("On this day strip", () => {
  const years = [
    { year: 2023, years_ago: 3, count: 9, items: [item(1)] },
    { year: 2025, years_ago: 1, count: 1, items: [item(2)] },
  ];

  it("shows a card per year, tapping opens the page of that year", async () => {
    vi.spyOn(api, "memories").mockResolvedValue({ enabled: true, day: "10-19", years });
    renderIn(<MemoriesStrip />);
    expect(await screen.findByRole("heading", { name: "On this day" })).toBeInTheDocument();
    const card = screen.getByRole("link", { name: "3 years ago, 9 photos" });
    expect(card).toHaveAttribute("href", "/memories#y2023");
    expect(screen.getByRole("link", { name: "1 year ago, 1 photo" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "See all" })).toHaveAttribute("href", "/memories");
  });

  it("draws nothing when there is nothing, when it is off, or when the request fails", async () => {
    const memories = vi.spyOn(api, "memories").mockResolvedValue({ enabled: true, day: "10-19", years: [] });
    const { container, unmount } = renderIn(<MemoriesStrip />);
    await waitFor(() => expect(memories).toHaveBeenCalled());
    expect(container.querySelector(".memories")).toBeNull();
    unmount();
    memories.mockResolvedValue({ enabled: false, day: "10-19", years });
    const off = renderIn(<MemoriesStrip />);
    await waitFor(() => expect(memories).toHaveBeenCalledTimes(2));
    expect(off.container.querySelector(".memories")).toBeNull();
    off.unmount();
    memories.mockRejectedValue(new ApiError(500, "boom"));
    const failed = renderIn(<MemoriesStrip />);
    await waitFor(() => expect(memories).toHaveBeenCalledTimes(3));
    expect(failed.container.querySelector(".memories")).toBeNull();
  });

  it("is hidden for good with Hide", async () => {
    vi.spyOn(api, "memories").mockResolvedValue({ enabled: true, day: "10-19", years });
    const set = vi.spyOn(api, "setPrefs").mockResolvedValue({ memories_enabled: false, library_sort: "taken", stacks_collapsed: true, grid_style: "square" });
    renderIn(<MemoriesStrip />);
    await userEvent.click(await screen.findByRole("button", { name: "Hide On this day" }));
    expect(set).toHaveBeenCalledWith({ memories_enabled: false });
    expect(screen.queryByRole("heading", { name: "On this day" })).toBeNull();
  });
});

describe("Library: sort, archive and hide", () => {
  it("offers Date taken / Date added and remembers the choice", async () => {
    fakeLibrary([item(1), item(2)]);
    const set = vi.spyOn(api, "setPrefs").mockResolvedValue({ memories_enabled: true, library_sort: "added", stacks_collapsed: true, grid_style: "square" });
    renderIn(<Library />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "View options" }));
    expect(await screen.findByRole("menuitemradio", { name: "Date taken" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{Escape}");
    await chooseView(user, "Date added");
    expect(set).toHaveBeenCalledWith({ library_sort: "added" });
    await user.click(screen.getByRole("button", { name: "View options" }));
    await waitFor(() => expect(screen.getByRole("menuitemradio", { name: "Date added" })).toHaveAttribute("aria-checked", "true"));
    expect(api.listAssets).toHaveBeenLastCalledWith(expect.objectContaining({ sort: "added" }));
  });

  it("starts in the saved order", async () => {
    const lib = fakeLibrary([item(1)]);
    vi.mocked(api.prefs).mockResolvedValue({ memories_enabled: true, library_sort: "added", stacks_collapsed: true, grid_style: "square" });
    renderIn(<Library />);
    await screen.findByAltText("photo-1.jpg");
    expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ sort: "added" }));
  });

  it("Recently added is by upload time whatever was saved, with no toggle", async () => {
    const lib = fakeLibrary([item(1)]);
    renderIn(<Library recent />, "/recent");
    await screen.findByAltText("photo-1.jpg");
    expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ sort: "added" }));
    expect(screen.getByRole("heading", { name: "Recently added" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "View options" }));
    expect(await screen.findByRole("menuitemradio", { name: "Square" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitemradio", { name: "Date taken" })).toBeNull();
  });

  it("archives the selection, takes it out of the grid and offers Undo", async () => {
    fakeLibrary([item(1), item(2), item(3)]);
    const user = userEvent.setup();
    renderIn(<Library />);
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    await pressBar(user, "Archive");
    expect(api.archiveAssets).toHaveBeenCalledWith(["id-1"], true);
    expect(await screen.findByText("1 archived")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByAltText("photo-1.jpg")).toBeNull());
    await user.click(screen.getByRole("button", { name: "Undo" }));
    expect(api.archiveAssets).toHaveBeenLastCalledWith(["id-1"], false);
    expect(await screen.findByText("1 back in the library")).toBeInTheDocument();
  });

  it("archives the whole view by scope and undoes exactly that batch", async () => {
    fakeLibrary([item(1), item(2)]);
    vi.mocked(api.archiveAssets).mockResolvedValueOnce({ count: 2, batch: "2026-01-01T00:00:00Z" });
    const user = userEvent.setup();
    renderIn(<Library />);
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    await pressBar(user, "Archive");
    expect(api.archiveAssets).toHaveBeenCalledWith({ scope: "library", except: [] }, true);
    await user.click(await screen.findByRole("button", { name: "Undo" }));
    expect(api.archiveAssets).toHaveBeenLastCalledWith(
      { scope: { archive_batch: "2026-01-01T00:00:00Z" }, except: [] },
      false,
    );
  });

  it("hides the selection and says where to find it", async () => {
    fakeLibrary([item(1), item(2)]);
    const user = userEvent.setup();
    renderIn(<Library />);
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-2.jpg" }));
    await pressBar(user, "Hide");
    expect(api.hideAssets).toHaveBeenCalledWith(["id-2"], true);
    expect(await screen.findByText(/1 hidden\. Find it under More, Hidden\./)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByAltText("photo-2.jpg")).toBeNull());
  });

  it("hides and archives from the viewer menu", async () => {
    fakeLibrary([item(1), item(2)]);
    const user = userEvent.setup();
    renderIn(<Library />);
    await user.click(await screen.findByRole("button", { name: "Open photo-1.jpg" }));
    await user.click(await screen.findByRole("button", { name: "Hide" }));
    expect(api.hideAssets).toHaveBeenCalledWith(["id-1"], true);
    await user.click(await screen.findByRole("button", { name: "Archive" }));
    expect(api.archiveAssets).toHaveBeenCalledWith(["id-2"], true);
  });

  it("the Archive view lists archived photos and restores them", async () => {
    const lib = fakeLibrary([item(1, { archived: true }), item(2)]);
    const user = userEvent.setup();
    renderIn(<Library archive />, "/archive");
    await screen.findByAltText("photo-1.jpg");
    expect(screen.queryByAltText("photo-2.jpg")).toBeNull();
    expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ archived: true }));
    expect(screen.getByRole("heading", { name: "Archive" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    await pressBar(user, "Restore");
    expect(api.archiveAssets).toHaveBeenCalledWith(["id-1"], false);
    expect(await screen.findByText("1 back in the library")).toBeInTheDocument();
  });

  it("the Archive view says so when empty", async () => {
    fakeLibrary([item(1)]);
    renderIn(<Library archive />, "/archive");
    expect(await screen.findByText("The archive is empty")).toBeInTheDocument();
  });

  it("the Hidden view unhides, and asks for the password again when the server says it is locked", async () => {
    fakeLibrary([item(1, { hidden: true })]);
    const user = userEvent.setup();
    const locked = vi.fn();
    renderIn(<Library hidden onLocked={locked} />, "/hidden");
    await screen.findByAltText("photo-1.jpg");
    expect(screen.queryByRole("button", { name: "Archive" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    await pressBar(user, "Unhide");
    expect(api.hideAssets).toHaveBeenCalledWith(["id-1"], false);
    expect(locked).not.toHaveBeenCalled();
  });

  it("calls onLocked when the hidden list is refused", async () => {
    fakeLibrary([]);
    vi.mocked(api.timeline).mockRejectedValue(new ApiError(403, "locked", "hidden_locked"));
    const locked = vi.fn();
    renderIn(<Library hidden onLocked={locked} />, "/hidden");
    await waitFor(() => expect(locked).toHaveBeenCalled());
  });
});

describe("Hidden page", () => {
  it("asks for the password, shows a wrong one, then opens", async () => {
    fakeLibrary([item(1, { hidden: true })]);
    vi.spyOn(api, "hiddenState").mockResolvedValue({ unlocked: false, expires_at: null });
    const unlock = vi
      .spyOn(api, "unlockHidden")
      .mockRejectedValueOnce(new ApiError(403, "That password is not right.", "wrong_password"))
      .mockResolvedValue({ unlocked: true, expires_at: new Date(Date.now() + 600_000).toISOString() });
    const user = userEvent.setup();
    renderIn(<HiddenPage />, "/hidden");
    const field = await screen.findByLabelText("Password");
    expect(screen.queryByAltText("photo-1.jpg")).toBeNull();
    await user.type(field, "nope");
    await user.click(screen.getByRole("button", { name: "Open Hidden" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("That password is not right.");
    expect(unlock).toHaveBeenCalledWith("nope");
    expect(screen.getByLabelText("Password")).toHaveValue("");
    await user.type(screen.getByLabelText("Password"), "right");
    await user.click(screen.getByRole("button", { name: "Open Hidden" }));
    expect(await screen.findByAltText("photo-1.jpg")).toBeInTheDocument();
    expect(screen.getByText(/Unlocked until/)).toBeInTheDocument();
  });

  it("goes straight in while the unlock is open, and Lock now closes it", async () => {
    fakeLibrary([item(1, { hidden: true })]);
    vi.spyOn(api, "hiddenState").mockResolvedValue({ unlocked: true, expires_at: new Date(Date.now() + 600_000).toISOString() });
    const lock = vi.spyOn(api, "lockHidden").mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderIn(<HiddenPage />, "/hidden");
    expect(await screen.findByAltText("photo-1.jpg")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Lock now" }));
    expect(lock).toHaveBeenCalled();
    expect(await screen.findByLabelText("Password")).toBeInTheDocument();
    expect(screen.queryByAltText("photo-1.jpg")).toBeNull();
  });

  it("asks again when the ten minutes run out", async () => {
    fakeLibrary([item(1, { hidden: true })]);
    vi.spyOn(api, "hiddenState").mockResolvedValue({ unlocked: true, expires_at: new Date(Date.now() + 1500).toISOString() });
    renderIn(<HiddenPage />, "/hidden");
    expect(await screen.findByAltText("photo-1.jpg")).toBeInTheDocument();
    expect(await screen.findByLabelText("Password", {}, { timeout: 5000 })).toBeInTheDocument();
  });
});
