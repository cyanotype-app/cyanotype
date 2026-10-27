import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type PetsSummary } from "../api";
import { AuthProvider } from "../auth";
import { fakeLibrary, type FakeLibrary } from "../test/fakeLibrary";
import { detail, item, namedPet } from "../test/fixtures";
import { PetsProvider } from "../usePets";
import { AppShell } from "../shell/AppShell";
import { UploadPickerProvider } from "../shell/UploadPicker";
import { UploadProvider } from "../upload/UploadProvider";
import { PetsPage } from "./PetsPage";

const cover = { asset_id: "id-1", box: [0.1, 0.1, 0.3, 0.3] as [number, number, number, number], preview_url: "https://s3.test/pet" };
const summary = (dogs: number, cats: number, over: Partial<PetsSummary> = {}): PetsSummary => ({
  any: dogs + cats,
  kinds: [
    { kind: "dog", photos: dogs, cover: dogs ? cover : null },
    { kind: "cat", photos: cats, cover: cats ? cover : null },
  ],
  scanned: 10,
  total: 10,
  ...over,
});

let lib: FakeLibrary;
beforeEach(() => {
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    },
  );
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co", role: "member" });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
  vi.spyOn(api, "namedPets").mockResolvedValue({ items: [] });
  lib = fakeLibrary([item(1), item(2), item(3)]);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderAt = (path = "/pets") =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <PetsProvider>
          <Routes>
            <Route path="/pets" element={<PetsPage />} />
            <Route path="/pets/:id" element={<p>a pet</p>} />
          </Routes>
        </PetsProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

describe("the Pets page", () => {
  it("is not there while the feature is off: not found, and nothing about pets is asked for", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: false });
    const pets = vi.spyOn(api, "pets");
    renderAt();
    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeInTheDocument();
    expect(pets).not.toHaveBeenCalled();
  });

  it("shows All, Dogs and Cats and the photos of the pets as the library's timeline", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 1));
    renderAt();
    const tabs = await screen.findByRole("radiogroup", { name: "Show" });
    expect(screen.getByRole("heading", { level: 1, name: "Pets" })).toBeInTheDocument();
    expect(within(tabs).getAllByRole("radio").map((r) => r.textContent)).toEqual(["All", "Dogs", "Cats"]);
    expect(await screen.findByRole("button", { name: "Open photo-1.jpg" })).toBeInTheDocument();
    expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ pets: "any" }));
    expect(lib.listAssets).toHaveBeenCalledWith(expect.objectContaining({ pets: "any" }));
  });

  it("switches to one kind, which reads its own timeline", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 1));
    renderAt();
    await userEvent.click(await screen.findByRole("radio", { name: "Cats" }));
    await waitFor(() => expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ pets: "cat" })));
    expect(screen.getByRole("radio", { name: "Cats" })).toBeChecked();
  });

  it("opens on the kind the link names", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 1));
    renderAt("/pets?kind=dog");
    await waitFor(() => expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ pets: "dog" })));
    expect(await screen.findByRole("radio", { name: "Dogs" })).toBeChecked();
  });

  it("does not offer a kind with no photos, and a link to it shows All", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 0));
    renderAt("/pets?kind=cat");
    await waitFor(() => expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ pets: "any" })));
    expect(lib.timeline).not.toHaveBeenCalledWith(expect.objectContaining({ pets: "cat" }));
    expect(screen.queryByRole("radio", { name: "Cats" })).not.toBeInTheDocument();
  });

  it("has no switch with a single kind", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(0, 0, { any: 3, kinds: [{ kind: "dog", photos: 3, cover }, { kind: "cat", photos: 0, cover: null }] }));
    renderAt();
    await screen.findByRole("button", { name: "Open photo-1.jpg" });
    expect(screen.queryByRole("radiogroup", { name: "Show" })).not.toBeInTheDocument();
  });

  it("selects the whole view for a bulk action by its scope", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 1));
    const fav = vi.spyOn(api, "favoriteAssets").mockResolvedValue({ count: 3 });
    renderAt("/pets?kind=dog");
    await userEvent.click(await screen.findByRole("button", { name: "Select" }));
    await userEvent.click(await screen.findByRole("button", { name: /^Select all/ }));
    await userEvent.click(screen.getByRole("button", { name: "Favorite" }));
    await waitFor(() => expect(fav).toHaveBeenCalled());
    expect(fav.mock.calls[0]![0]).toEqual({ scope: { pets: "dog" }, except: [] });
  });

  it("saves the view as an album, from the menu", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 1));
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "More" }));
    expect(await screen.findByRole("menuitem", { name: "Search with filters" })).toHaveAttribute("href", "/search?pets=any");
    await userEvent.click(screen.getByRole("menuitem", { name: "Save as album" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("says it is still looking, with how far it has got, while nothing is found yet", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(0, 0, { scanned: 1200, total: 5000 }));
    renderAt();
    expect(await screen.findByRole("heading", { name: "Looking for dogs and cats" })).toBeInTheDocument();
    expect(screen.getByText("Looked at 1,200 of 5,000 photos")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Looking for pets" })).toHaveAttribute("aria-valuenow", "24");
    expect(lib.timeline).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "More" })).not.toBeInTheDocument();
  });

  it("says none were found once the whole library has been looked at", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(0, 0));
    renderAt();
    expect(await screen.findByRole("heading", { name: "No pets found" })).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("keeps showing the progress above the photos while the rest is looked through", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 1, { scanned: 50, total: 100 }));
    renderAt();
    const region = await screen.findByRole("region", { name: "Looking for pets" });
    expect(within(region).getByText(/Looked at 50 of 100 photos/)).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Open photo-1.jpg" })).toBeInTheDocument();
  });

  it("offers a retry when the counts cannot be read", async () => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    // The first read is the shell's (its failure only leaves the entry points out), the second the page's.
    const pets = vi.spyOn(api, "pets").mockRejectedValueOnce(new Error("boom")).mockRejectedValueOnce(new Error("boom")).mockResolvedValue(summary(1, 0));
    renderAt();
    expect(await screen.findByRole("heading", { name: "Couldn’t load Pets" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("button", { name: "Open photo-1.jpg" })).toBeInTheDocument();
    expect(pets).toHaveBeenCalledTimes(3);
  });
});

describe("Your pets", () => {
  const on = (named = [namedPet("p1", "Pet A", 4, { pending: 2 }), namedPet("p2", "Pet B", 1, { kind: "cat", hidden: true })]) => {
    vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "pets").mockResolvedValue(summary(2, 1));
    vi.spyOn(api, "namedPets").mockResolvedValue({ items: named });
  };

  it("lists the pets that were named above the photos, hidden ones last, with what waits for an answer", async () => {
    on([namedPet("p2", "Pet B", 1, { kind: "cat", hidden: true }), namedPet("p1", "Pet A", 4, { pending: 2 })]);
    renderAt();
    const row = await screen.findByRole("region", { name: "Your pets" });
    const links = await within(row).findAllByRole("link");
    expect(links.map((l) => l.getAttribute("aria-label"))).toEqual(["Pet A, 4 photos, 2 photos to confirm", "Pet B, 1 photo"]);
    expect(links[0]).toHaveAttribute("href", "/pets/p1");
    expect(within(row).getByText("Hidden")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Open photo-1.jpg" })).toBeInTheDocument();
  });

  it("says how to name a pet while there is none", async () => {
    on([]);
    renderAt();
    const row = await screen.findByRole("region", { name: "Your pets" });
    expect(within(row).getByText(/Pick a few photos of your dog or cat/)).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: "Name a pet" })).toBeInTheDocument();
  });

  it("names a pet from photos picked after 'Name a pet' turned selection on", async () => {
    on([]);
    const create = vi.spyOn(api, "createPet").mockResolvedValue({ pet: namedPet("p9", "Pet C", 2), assigned: 2, ambiguous: [], skipped: [] });
    const user = userEvent.setup();
    renderAt();
    await user.click(await screen.findByRole("button", { name: "Name a pet" }));
    expect(await screen.findByText("Select photos of your pet")).toBeInTheDocument();
    const bar = screen.getByRole("toolbar", { name: "Selection" });
    expect(within(bar).getByRole("button", { name: "Name pet…" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Select photo-1.jpg" }));
    await user.click(screen.getByRole("button", { name: "Select photo-2.jpg" }));
    await user.click(within(bar).getByRole("button", { name: "Name pet…" }));
    const dialog = await screen.findByRole("dialog", { name: "Name pet" });
    await user.type(within(dialog).getByLabelText("Name"), "Pet C");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ name: "Pet C", kind: "dog", asset_ids: ["id-1", "id-2"] }));
    expect(await screen.findByText("Made Pet C with 2 photos")).toBeInTheDocument();
    expect(screen.queryByRole("toolbar", { name: "Selection" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open" }));
    expect(await screen.findByText("a pet")).toBeInTheDocument();
  });

  it("names a pet from the menu too, and not from a whole-view selection", async () => {
    on([]);
    const user = userEvent.setup();
    renderAt();
    await user.click(await screen.findByRole("button", { name: "More" }));
    await user.click(await screen.findByRole("menuitem", { name: "Name a pet" }));
    const bar = await screen.findByRole("toolbar", { name: "Selection" });
    await user.click(within(bar).getByRole("button", { name: "Select all" }));
    expect(within(bar).getByRole("button", { name: "Name pet…" })).toBeDisabled();
  });
});

describe("the Pets entry of the sidebar", () => {
  const renderShell = (status: "on" | "off" | "error", any = 4) => {
    if (status === "error") vi.spyOn(api, "petsStatus").mockRejectedValue(new Error("down"));
    else vi.spyOn(api, "petsStatus").mockResolvedValue({ enabled: status === "on" });
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: false });
    vi.spyOn(api, "pets").mockResolvedValue({ any, kinds: [], scanned: 5, total: 5 });
    render(
      <MemoryRouter initialEntries={["/"]}>
        <AuthProvider>
          <PetsProvider>
            <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
              <UploadPickerProvider>
                <AppShell>
                  <p>page</p>
                </AppShell>
              </UploadPickerProvider>
            </UploadProvider>
          </PetsProvider>
        </AuthProvider>
      </MemoryRouter>,
    );
  };
  const library = () => within(screen.getByRole("navigation", { name: "Library" }));

  it("is there when Pets is on and a photo has a pet", async () => {
    renderShell("on");
    await waitFor(() => expect(library().getByRole("link", { name: "Pets" })).toHaveAttribute("href", "/pets"));
  });

  it("is not there while no photo has a pet yet", async () => {
    renderShell("on", 0);
    await waitFor(() => expect(api.pets).toHaveBeenCalled());
    expect(library().getByRole("link", { name: "Tags" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Pets" })).not.toBeInTheDocument();
  });

  it.each(["off", "error"] as const)("is nowhere when Pets is %s", async (status) => {
    renderShell(status);
    await waitFor(() => expect(api.petsStatus).toHaveBeenCalled());
    expect(library().getByRole("link", { name: "Tags" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Pets" })).not.toBeInTheDocument();
    expect(api.pets).not.toHaveBeenCalled();
  });
});
