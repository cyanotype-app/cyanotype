import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type Facets, type SmartPreview, type SmartRules } from "../api";
import { AuthProvider } from "../auth";
import { album, item } from "../test/fixtures";
import { AlbumPage } from "../pages/Album";
import { Albums } from "../pages/Albums";
import { Route, Routes } from "react-router-dom";
import { SmartAlbumEditor } from "./SmartAlbumEditor";

afterEach(() => vi.restoreAllMocks());

const facets: Facets = {
  total: 10,
  favorites: 1,
  with_location: 1,
  types: [],
  cameras: [{ value: "Canon EOS R5", count: 3 }],
  lenses: [],
  places: [{ value: "Lisbon", kind: "city", count: 4 }],
  years: [],
};
const previewOf = (count: number, rules: SmartRules): SmartPreview => ({ count, capped: false, items: [item(1), item(2)], rules });
const noFacets = async () => facets;

describe("SmartAlbumEditor", () => {
  it("builds the rules, previews them live and saves a cleaned copy with the title", async () => {
    const user = userEvent.setup();
    const preview = vi.fn(async (rules: SmartRules) => previewOf(rules.rules.length * 7, rules));
    const onSubmit = vi.fn();
    render(
      <SmartAlbumEditor
        title="New smart album"
        submitLabel="Create smart album"
        initialRules={{ match: "all", archived: "include", rules: [{ field: "favorite", value: true }] }}
        onSubmit={onSubmit}
        onCancel={() => {}}
        preview={preview}
        loadFacets={noFacets}
      />,
    );
    expect(screen.getByRole("button", { name: "Create smart album" })).toBeDisabled(); // no title yet
    await user.type(screen.getByLabelText("Title"), "Lisbon favorites");
    expect(await screen.findByText(/7 photos/)).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("Kind of rule to add"), "date");
    await user.click(screen.getByRole("button", { name: "Add rule" }));
    // A date rule with no day is a problem, said in place of the preview.
    expect(screen.getByRole("button", { name: "Create smart album" })).toBeDisabled();
    expect(screen.getByText(/start or an end/)).toBeInTheDocument();
    await user.type(screen.getByLabelText("Rule 2: from"), "2024-01-01");
    expect(await screen.findByText(/14 photos/)).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("Kind of rule to add"), "place");
    await user.click(screen.getByRole("button", { name: "Add rule" }));
    await user.type(screen.getByRole("combobox", { name: "Place" }), "Lisbon");
    await user.click(screen.getByRole("radio", { name: "Any of the rules" }));
    await user.click(screen.getByRole("checkbox", { name: "Include archived photos" }));
    await user.click(screen.getByRole("button", { name: "Create smart album" }));
    expect(onSubmit).toHaveBeenCalledWith("Lisbon favorites", {
      match: "any",
      archived: "exclude",
      rules: [{ field: "favorite", value: true }, { field: "date", from: "2024-01-01" }, { field: "place", value: "Lisbon" }],
    });
  });

  it("removes a rule, says that a text search is not a rule, and Escape cancels", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    render(
      <SmartAlbumEditor
        title="Edit rules"
        submitLabel="Save"
        initialTitle="Mine"
        initialRules={{ match: "all", archived: "include", rules: [{ field: "favorite", value: true }, { field: "rating", min: 4 }] }}
        onSubmit={() => {}}
        onCancel={onCancel}
        preview={async (r) => previewOf(1, r)}
        loadFacets={noFacets}
      />,
    );
    expect(screen.getByText(/ranking is not stable/)).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Caption contains" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /search/i })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove rule 1" }));
    expect(within(screen.getByRole("list", { name: "Rules" })).getAllByRole("listitem")).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Remove rule 1" }));
    expect(screen.getByText("Add at least one rule.")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalled();
  });

  it("shows the server's refusal of a preview", async () => {
    render(
      <SmartAlbumEditor
        title="x"
        submitLabel="Save"
        initialRules={{ match: "all", archived: "include", rules: [{ field: "favorite", value: true }] }}
        onSubmit={() => {}}
        onCancel={() => {}}
        preview={() => Promise.reject(new Error("the album's rules are not valid"))}
        loadFacets={noFacets}
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("not valid");
  });
});

const smartAlbum = album(1, {
  title: "Faves and Lisbon",
  smart: true,
  rules: { match: "any", archived: "exclude", rules: [{ field: "favorite", value: true }, { field: "place", value: "Lisbon" }] },
  photo_count: 10000,
  photo_count_capped: true,
});

describe("Albums with a smart one", () => {
  it("badges it, counts it with a plus when capped, shows its rules, and offers New smart album", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [smartAlbum, album(2, { title: "Plain" })] });
    vi.spyOn(api, "searchFacets").mockResolvedValue(facets);
    vi.spyOn(api, "previewSmartAlbum").mockResolvedValue(previewOf(3, smartAlbum.rules!));
    const create = vi.spyOn(api, "createSmartAlbum").mockResolvedValue(smartAlbum);
    render(
      <MemoryRouter>
        <AuthProvider>
          <Albums />
        </AuthProvider>
      </MemoryRouter>,
    );
    const card = (await screen.findByRole("link", { name: /Faves and Lisbon/ })) as HTMLElement;
    expect(within(card).getByText("Smart")).toBeInTheDocument();
    expect(within(card).getByText(/10,000\+ photos/)).toBeInTheDocument();
    expect(within(card).getByText("Favorites or Place: Lisbon (without archived)")).toBeInTheDocument();
    expect(within(screen.getByRole("link", { name: /Plain/ })).queryByText("Smart")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "New smart album" }));
    await user.type(await screen.findByLabelText("Title"), "New one");
    await waitFor(() => expect(screen.getByRole("button", { name: "Create smart album" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Create smart album" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ title: "New one", rules: { match: "all", archived: "include", rules: [{ field: "favorite", value: true }] } }));
  });
});

describe("a smart album's page", () => {
  it("has Edit rules instead of Add photos, no cover or remove tools, and saves new rules", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
    vi.spyOn(api, "getAlbum").mockResolvedValue({ ...smartAlbum, photo_count: 2, photo_count_capped: false });
    vi.spyOn(api, "listAlbumAssets").mockResolvedValue({ items: [item(1), item(2)], next_cursor: null, next_before: null });
    vi.spyOn(api, "searchFacets").mockResolvedValue(facets);
    vi.spyOn(api, "previewSmartAlbum").mockResolvedValue(previewOf(2, smartAlbum.rules!));
    const update = vi.spyOn(api, "updateAlbum").mockResolvedValue({ ...smartAlbum, title: "Renamed", photo_count: 2 });
    vi.stubGlobal("IntersectionObserver", class { observe() {} disconnect() {} unobserve() {} });
    render(
      <MemoryRouter initialEntries={["/albums/album-1"]}>
        <AuthProvider>
          <Routes>
            <Route path="/albums/:id" element={<AlbumPage />} />
          </Routes>
        </AuthProvider>
      </MemoryRouter>,
    );
    expect(await screen.findByRole("heading", { name: /Faves and Lisbon/ })).toBeInTheDocument();
    expect(screen.getByText("Smart")).toBeInTheDocument();
    expect(screen.getByText(/Favorites or Place: Lisbon/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add photos" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Select" }));
    expect(screen.queryByRole("button", { name: "Set as cover" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove from album" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rate" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Edit rules" }));
    const title = await screen.findByLabelText("Title");
    await user.clear(title);
    await user.type(title, "Renamed");
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith("album-1", { title: "Renamed", rules: smartAlbum.rules }));
    vi.unstubAllGlobals();
  });
});
