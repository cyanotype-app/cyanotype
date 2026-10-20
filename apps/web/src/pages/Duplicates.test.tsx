import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type DuplicateGroup, type DuplicateMember, type DuplicatesPage } from "../api";
import { AuthProvider } from "../auth";
import { item } from "../test/fixtures";
import { Duplicates } from "./Duplicates";
import { keepLabel, keptMessage, metadataLine, resolutionLine, scanLine, sizeLine } from "../dupText";

const member = (n: number, over: Partial<DuplicateMember> = {}): DuplicateMember => ({
  ...item(n),
  exif_fields: 12,
  has_location: false,
  has_caption: false,
  tag_count: 0,
  best: false,
  ...over,
});
const group = (id: string, members: DuplicateMember[]): DuplicateGroup => ({ id, members });
const page = (groups: DuplicateGroup[], over: Partial<DuplicatesPage> = {}): DuplicatesPage => ({
  groups,
  total: groups.length,
  next_cursor: null,
  scan: { state: "idle", groups: groups.length, photos: 100, finished_at: new Date().toISOString() },
  ...over,
});

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co", role: "member" });
});
afterEach(() => vi.restoreAllMocks());

const renderPage = () =>
  render(
    <MemoryRouter>
      <AuthProvider>
        <Duplicates />
      </AuthProvider>
    </MemoryRouter>,
  );

describe("the duplicate words", () => {
  it("says resolution, metadata, size and the state of the check", () => {
    expect(resolutionLine(4000, 3000)).toBe("4000 × 3000 · 12 MP");
    expect(resolutionLine(1000, 750)).toBe("1000 × 750 · 0.8 MP");
    expect(resolutionLine(null, 3)).toBe("Unknown size");
    expect(metadataLine({ exif_fields: 34, has_location: true, has_caption: true, tag_count: 2 })).toBe("34 EXIF fields, location, caption, 2 tags");
    expect(metadataLine({ exif_fields: 1, has_location: false, has_caption: false, tag_count: 1 })).toBe("1 EXIF field, 1 tag");
    expect(metadataLine({ exif_fields: 0, has_location: false, has_caption: false, tag_count: 0 })).toBe("No metadata");
    const all = [{ size_bytes: 5_000_000 }, { size_bytes: 90_000 }];
    expect(sizeLine(all[0]!, all)).toBe("4.8 MB");
    expect(sizeLine(all[1]!, all)).toBe("88 KB (2 % of the largest)");
    const now = new Date("2026-01-01T12:00:00Z");
    expect(scanLine(undefined, now)).toMatch(/not been checked/);
    expect(scanLine({ state: "running" }, now)).toMatch(/Checking/);
    expect(scanLine({ state: "wanted", manual: false }, now)).toMatch(/after your latest uploads/);
    expect(scanLine({ state: "idle", groups: 0, photos: 5, finished_at: "2026-01-01T11:57:00Z" }, now)).toBe("Last checked 3 min ago (5 photos): no duplicates found.");
    expect(scanLine({ state: "idle", groups: 2, photos: 1500, finished_at: "2025-12-30T12:00:00Z" }, now)).toBe("Last checked 2 d ago (1,500 photos): 2 groups found.");
    expect(scanLine({ state: "idle", error: "boom" }, now)).toBe("The last check failed: boom");
    expect(keepLabel(1)).toBe("Keep selected, trash 1 other");
    expect(keepLabel(3)).toBe("Keep selected, trash 3 others");
    expect(keptMessage({ trashed: ["a", "b"], merged: { albums: 1, tags: 2, favorite: true, rating: 5, caption: true, location: true } })).toBe(
      "2 photos moved to the trash. Merged into the one you kept: 1 album, 2 tags, the favorite, 5 stars, captions, the location.",
    );
    expect(keptMessage({ trashed: ["a"], merged: { albums: 0, tags: 0 } })).toBe("1 photo moved to the trash.");
  });
});

describe("Duplicates page", () => {
  it("shows groups side by side, preselects the best, keeps the chosen one and offers Undo", async () => {
    const user = userEvent.setup();
    const g = group("g1", [member(1, { width: 1000, height: 750, size_bytes: 90_000 }), member(2, { width: 4000, height: 3000, size_bytes: 5_000_000, best: true })]);
    vi.spyOn(api, "duplicates").mockResolvedValue(page([g]));
    const keep = vi.spyOn(api, "keepDuplicate").mockResolvedValue({ kept: "id-2", trashed: ["id-1"], merged: { albums: 1 } });
    const restore = vi.spyOn(api, "restoreAssets").mockResolvedValue({ count: 1 });
    renderPage();
    const list = await screen.findByRole("list", { name: "Duplicate groups" });
    expect(within(list).getByText("Group 1 · 2 photos")).toBeInTheDocument();
    expect(within(list).getByText("4000 × 3000 · 12 MP")).toBeInTheDocument();
    expect(within(list).getByText("1000 × 750 · 0.8 MP")).toBeInTheDocument();
    expect(within(list).getByText("Suggested")).toBeInTheDocument();
    const radios = within(list).getAllByRole("radio");
    expect(radios.map((r) => (r as HTMLInputElement).checked)).toEqual([false, true]);
    // The best is kept as it is; choosing the other changes what is sent.
    await user.click(radios[0]!);
    expect(screen.getByRole("button", { name: "Keep selected, trash 1 other" })).toBeInTheDocument();
    await user.click(radios[1]!);
    await user.click(screen.getByRole("button", { name: "Keep selected, trash 1 other" }));
    expect(keep).toHaveBeenCalledWith("g1", "id-2");
    expect(await screen.findByText(/1 photo moved to the trash/)).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Duplicate groups" })?.children.length ?? 0).toBe(0);
    await user.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(restore).toHaveBeenCalledWith(["id-1"]));
    expect(await screen.findByText(/What was merged onto the kept photo stays/)).toBeInTheDocument();
  });

  it("sends the first photo when the user picks it, and dismisses a group", async () => {
    const user = userEvent.setup();
    const g1 = group("g1", [member(1, { best: true }), member(2)]);
    const g2 = group("g2", [member(3, { best: true }), member(4)]);
    vi.spyOn(api, "duplicates").mockResolvedValue(page([g1, g2]));
    const keep = vi.spyOn(api, "keepDuplicate").mockResolvedValue({ kept: "id-2", trashed: ["id-1"], merged: {} });
    const dismiss = vi.spyOn(api, "dismissDuplicates").mockResolvedValue({ dismissed: 2 });
    renderPage();
    await screen.findByText("Group 1 · 2 photos");
    const groups = screen.getByRole("list", { name: "Duplicate groups" });
    const [one, two] = within(groups).getAllByRole("listitem", { hidden: false }).filter((li) => li.classList.contains("dup-group"));
    await user.click(within(one!).getAllByRole("radio")[1]!);
    await user.click(within(one!).getByRole("button", { name: /Keep selected/ }));
    expect(keep).toHaveBeenCalledWith("g1", "id-2");
    await user.click(await within(two!).findByRole("button", { name: "Not duplicates" }));
    expect(dismiss).toHaveBeenCalledWith("g2");
    expect(await screen.findByText(/will not be offered again/)).toBeInTheDocument();
  });

  it("starts a check, polls while it runs and shows the groups it finds", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const found = group("g1", [member(1, { best: true }), member(2)]);
    const list = vi
      .spyOn(api, "duplicates")
      .mockResolvedValueOnce(page([], { scan: { state: "never" } }))
      .mockResolvedValueOnce(page([], { scan: { state: "running" } }))
      .mockResolvedValue(page([found]));
    const scan = vi.spyOn(api, "scanDuplicates").mockResolvedValue({ scan: { state: "wanted", manual: true } });
    renderPage();
    expect(await screen.findByText("Nothing checked yet")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Find duplicates" }));
    expect(scan).toHaveBeenCalled();
    expect(await screen.findByRole("button", { name: "Checking…" })).toBeDisabled();
    await vi.advanceTimersByTimeAsync(3100);
    await vi.advanceTimersByTimeAsync(3100);
    expect(await screen.findByText("Group 1 · 2 photos")).toBeInTheDocument();
    expect(list.mock.calls.length).toBeGreaterThanOrEqual(3);
    vi.useRealTimers();
  });

  it("offers the admin check for every account, and loads more groups", async () => {
    const user = userEvent.setup();
    vi.mocked(api.me).mockResolvedValue({ id: "u", email: "a@b.co", role: "admin" });
    const g = (n: number) => group(`g${n}`, [member(n * 2, { best: true }), member(n * 2 + 1)]);
    const spy = vi
      .spyOn(api, "duplicates")
      .mockResolvedValueOnce(page([g(1)], { total: 2, next_cursor: "c1" }))
      .mockResolvedValueOnce(page([g(2)], { total: 2 }));
    const all = vi.spyOn(api, "scanAllDuplicates").mockResolvedValue({ accounts: 3 });
    renderPage();
    await screen.findByText("Group 1 · 2 photos");
    await user.click(await screen.findByRole("button", { name: "Show more groups" }));
    expect(spy).toHaveBeenLastCalledWith({ cursor: "c1", limit: 20 });
    expect(await screen.findByText("Group 2 · 2 photos")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Check every account" }));
    expect(all).toHaveBeenCalled();
    expect(await screen.findByText("Checking 3 accounts.")).toBeInTheDocument();
  });
});
