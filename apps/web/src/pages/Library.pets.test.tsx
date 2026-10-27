// "Name pet…" in the Photos selection bar: only while Pets is on, from the photos picked one by one.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { UploadPickerProvider } from "../shell/UploadPicker";
import { SlideshowProvider } from "../slideshow/SlideshowHost";
import { barDisabled, pressBar } from "../test/bar";
import { fakeLibrary } from "../test/fakeLibrary";
import { detail, item, namedPet } from "../test/fixtures";
import { UploadProvider } from "../upload/UploadProvider";
import { PetsProvider } from "../usePets";
import { Library } from "./Library";

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
  vi.spyOn(api, "namedPets").mockResolvedValue({ items: [] });
  localStorage.clear();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderLibrary = () =>
  render(
    <MemoryRouter initialEntries={["/photos"]}>
      <AuthProvider>
        <PetsProvider>
          <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
            <UploadPickerProvider>
              <SlideshowProvider>
                <Routes>
                  <Route path="/photos" element={<Library />} />
                  <Route path="/pets/:id" element={<p>a pet</p>} />
                </Routes>
              </SlideshowProvider>
            </UploadPickerProvider>
          </UploadProvider>
        </PetsProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

const photos = [1, 2, 3].map((n) => item(n, { sort_time: new Date(Date.UTC(2026, 0, 30, 23 - n)).toISOString() }));

describe("Name pet… in the Photos selection", () => {
  it("makes a pet of the photos picked, and the toast opens its page", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue({ any: 3, kinds: [], scanned: 3, total: 3 });
    fakeLibrary(photos);
    const create = vi.spyOn(api, "createPet").mockResolvedValue({ pet: namedPet("p9", "Pet C", 2), assigned: 2, ambiguous: [], skipped: [] });
    const user = userEvent.setup();
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    // Nothing picked: off. The whole library selected: off too (it needs the photos one by one).
    await waitFor(async () => expect(await barDisabled(user, "Name pet…")).toBe(true));
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    await user.click(screen.getByRole("button", { name: "Select photo-2.jpg" }));
    await pressBar(user, "Name pet…");
    const dialog = await screen.findByRole("dialog", { name: "Name pet" });
    await user.type(within(dialog).getByLabelText("Name"), "Pet C");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ name: "Pet C", kind: "dog", asset_ids: ["id-1", "id-2"] }));
    expect(await screen.findByText("Made Pet C with 2 photos")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open" }));
    expect(await screen.findByText("a pet")).toBeInTheDocument();
  });

  it("is not there while Pets is off", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: false });
    fakeLibrary(photos);
    const user = userEvent.setup();
    renderLibrary();
    await screen.findByAltText("photo-1.jpg");
    await waitFor(() => expect(api.petsStatus).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "Select" }));
    const bar = screen.getByRole("toolbar", { name: "Selection" });
    await user.click(within(bar).getByRole("button", { name: "More actions" }));
    await screen.findByRole("menu");
    expect(screen.queryByRole("menuitem", { name: "Name pet…" })).not.toBeInTheDocument();
    expect(within(bar).queryByRole("button", { name: "Name pet…" })).not.toBeInTheDocument();
  });
});
