import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type FaceSuggestion } from "../api";
import { AuthProvider } from "../auth";
import { fakeLibrary, type FakeLibrary } from "../test/fakeLibrary";
import { detail, item, person } from "../test/fixtures";
import { PeopleProvider } from "../usePeople";
import { PersonPage } from "./PersonPage";

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
  vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
  vi.spyOn(api, "personSuggestions").mockResolvedValue({ items: [] });
  // The person's page is the library's timeline over the person's own endpoints: the fake serves the same photos either way.
  lib = fakeLibrary([item(1), item(2), item(3)]);
});
let lib: FakeLibrary;
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const renderAt = (path = "/people/p1") =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <PeopleProvider>
          <Routes>
            <Route path="/people" element={<p>all people</p>} />
            <Route path="/people/:id" element={<PersonPage />} />
          </Routes>
        </PeopleProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

const open = (n: number) => screen.getByRole("button", { name: new RegExp(`^(Open|Select) photo-${n}\\.jpg$`) });
const suggestion = (n: number): FaceSuggestion => ({ face_id: `s${n}`, asset_id: `a${n}`, box: [0.1, 0.1, 0.2, 0.2], preview_url: `https://s3.test/p${n}`, similarity: 0.7 });

describe("a person's page", () => {
  it("shows the name, the count and the photos, with a way back", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    renderAt();
    expect(await screen.findByRole("heading", { name: "Person A" })).toBeInTheDocument();
    expect(screen.getByText("3 photos")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Open photo-1.jpg" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "‹ People" })).toHaveAttribute("href", "/people");
    expect(lib.timeline).toHaveBeenCalledWith(expect.objectContaining({ person: "p1" }));
    expect(lib.listAssets).toHaveBeenCalledWith(expect.objectContaining({ person: "p1" }));
  });

  it("is a timeline: month and day headers, and the count of the photos", async () => {
    lib.photos = [item(1, { sort_time: "2026-10-14T09:00:00Z" }), item(2, { sort_time: "2026-10-02T07:00:00Z" }), item(3, { sort_time: "2025-12-31T23:59:59Z" })];
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    renderAt();
    await screen.findByAltText("photo-1.jpg");
    expect(screen.getAllByRole("heading", { level: 2 }).map((h) => h.textContent)).toEqual(["October 2026", "December 2025"]);
    expect(screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["Wed, Oct 14", "Fri, Oct 2", "Wed, Dec 31"]);
  });

  it("has the library's scrubber when there is a lot to scroll", async () => {
    lib.photos = Array.from({ length: 300 }, (_, i) => item(i + 1, { sort_time: new Date(Date.UTC(2026, 2, 30) - i * 20 * 3_600_000).toISOString() }));
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 300));
    renderAt();
    await screen.findByAltText("photo-1.jpg");
    expect(await screen.findByRole("slider", { name: /by date/i })).toBeInTheDocument();
  });

  it("reads the timeline again when the worker has grouped new photos", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const get = vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
      renderAt();
      await screen.findByAltText("photo-1.jpg");
      const before = lib.timeline.mock.calls.length;
      // A new photo is grouped into the person meanwhile: the counts change, and the page follows.
      lib.photos.push(item(4, { sort_time: "2026-01-02T00:00:00Z" }));
      get.mockResolvedValue(person("p1", "Person A", 4));
      await vi.advanceTimersByTimeAsync(15_500);
      expect(await screen.findByAltText("photo-4.jpg")).toBeInTheDocument();
      expect(lib.timeline.mock.calls.length).toBeGreaterThan(before);
      expect(screen.getAllByText("4 photos")).toHaveLength(2); // the head and the foot of the page
    } finally {
      vi.useRealTimers();
    }
  });

  it("selects every photo of the person without listing ids and works through the {person} scope", async () => {
    lib.photos = Array.from({ length: 250 }, (_, i) => item(i + 1, { sort_time: `2026-03-${String(1 + Math.floor(i / 10)).padStart(2, "0")}T12:00:00Z` }));
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 250));
    const user = userEvent.setup();
    renderAt();
    await screen.findByAltText("photo-250.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    expect(screen.getByText("250 selected")).toBeInTheDocument();
    // Taking photos out one by one needs ids, so it is off for the whole person.
    expect(screen.getByRole("button", { name: "Not this person" })).toBeDisabled();
    await user.click(open(250));
    expect(screen.getByText("249 selected")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Favorite" }));
    expect(await screen.findByText("249 added to favorites")).toBeInTheDocument();
    expect(api.favoriteAssets).toHaveBeenCalledWith({ scope: { person: "p1" }, except: ["id-250"] }, true);

    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: "Move to trash" }));
    expect(screen.getByRole("heading", { name: "Move 250 photos to trash?" })).toBeInTheDocument();
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Move to trash" }));
    expect(await screen.findByText("250 moved to trash")).toBeInTheDocument();
    expect(api.trashAssets).toHaveBeenCalledWith({ scope: { person: "p1" }, except: [] });
  });

  it("archives and hides through the scope", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    const user = userEvent.setup();
    renderAt();
    await screen.findByAltText("photo-1.jpg");
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(api.archiveAssets).toHaveBeenCalledWith({ scope: { person: "p1" }, except: [] }, true));
    await user.click(screen.getByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: "Hide photos" }));
    await waitFor(() => expect(api.hideAssets).toHaveBeenCalledWith({ scope: { person: "p1" }, except: [] }, true));
    expect(await screen.findByText(/hidden\. Find them under More, Hidden/)).toBeInTheDocument();
  });

  it("saves the person's photos as a normal album from the scope, with a title that is not their name", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    const create = vi.spyOn(api, "createAlbum").mockResolvedValue({ id: "al1", title: "Holiday", description: null, photo_count: 0, cover: null, first_at: null, last_at: null, created_at: "", updated_at: "" });
    const add = vi.spyOn(api, "addToAlbum").mockResolvedValue({ count: 3 });
    const list = vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    const user = userEvent.setup();
    renderAt();
    await user.click(await screen.findByRole("button", { name: "Save as album" }));
    const dialog = screen.getByRole("dialog", { name: "Save 3 photos as an album" });
    // It starts at the title (nothing to pick from), empty: the person's name is not offered as the album's title.
    const title = within(dialog).getByLabelText("Album title");
    expect(title).toHaveValue("");
    expect(dialog).toHaveTextContent(/people you share it with can see/);
    expect(list).not.toHaveBeenCalled();
    await user.type(title, "Holiday");
    await user.click(within(dialog).getByRole("button", { name: "Create album" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith({ title: "Holiday" }));
    expect(add).toHaveBeenCalledWith("al1", { scope: { person: "p1" }, except: [] });
    expect(await screen.findByText("Created Holiday with 3 photos")).toBeInTheDocument();
  });

  it("links to the search with the person as a filter", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    renderAt();
    expect(await screen.findByRole("link", { name: "Search with filters" })).toHaveAttribute("href", "/search?people=p1");
  });

  it("says so for a person who is not there, and for the feature being off", async () => {
    vi.spyOn(api, "getPerson").mockRejectedValue(new ApiError(404, "not found"));
    renderAt();
    expect(await screen.findByRole("heading", { name: "Person not found" })).toBeInTheDocument();
    vi.restoreAllMocks();
    vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co", role: "member" });
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: false });
    const get = vi.spyOn(api, "getPerson");
    renderAt();
    expect(await screen.findAllByRole("heading", { name: "Page not found" })).not.toHaveLength(0);
    expect(get).not.toHaveBeenCalled();
  });

  it("renames in place, and a blank name clears it", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", null, 3));
    const update = vi.spyOn(api, "updatePerson").mockResolvedValue(person("p1", "Person A", 3));
    renderAt();
    expect(await screen.findByRole("heading", { name: "Unnamed person" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Add a name" }));
    await userEvent.type(screen.getByLabelText("Name"), "  Person A ");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(update).toHaveBeenCalledWith("p1", { name: "Person A" });
    expect(await screen.findByRole("heading", { name: "Person A" })).toBeInTheDocument();
    update.mockResolvedValue(person("p1", null, 3));
    await userEvent.click(screen.getByRole("button", { name: "Rename" }));
    await userEvent.clear(screen.getByLabelText("Name"));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(update).toHaveBeenLastCalledWith("p1", { name: null });
    expect(await screen.findByRole("heading", { name: "Unnamed person" })).toBeInTheDocument();
  });

  it("changes nothing when a rename is cancelled", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    const update = vi.spyOn(api, "updatePerson");
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Rename" }));
    const input = screen.getByLabelText("Name");
    expect(input).toHaveValue("Person A");
    await userEvent.type(input, " and more");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(update).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Person A" })).toBeInTheDocument();
  });

  it("hides and shows the person", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    const update = vi.spyOn(api, "updatePerson").mockResolvedValue(person("p1", "Person A", 3, { hidden: true }));
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Hide" }));
    expect(update).toHaveBeenCalledWith("p1", { hidden: true });
    expect(await screen.findByRole("button", { name: "Show" })).toBeInTheDocument();
    expect(screen.getByText(/Hidden/, { selector: "p" })).toBeInTheDocument();
  });

  it("takes selected photos out with 'Not this person', after asking", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    const remove = vi.spyOn(api, "removeFromPerson").mockResolvedValue({ removed: 2 });
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Select" }));
    expect(screen.getByRole("button", { name: "Not this person" })).toBeDisabled();
    await userEvent.click(open(1));
    await userEvent.click(open(3));
    await userEvent.click(screen.getByRole("button", { name: "Not this person" }));
    const dialog = screen.getByRole("alertdialog", { name: "Not Person A?" });
    expect(dialog).toHaveTextContent("taken out of 2 photos");
    await userEvent.click(within(dialog).getByRole("button", { name: "Not this person" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("p1", { asset_ids: ["id-1", "id-3"] }));
    expect(await screen.findByText("Took 2 photos out of Person A")).toBeInTheDocument();
    await waitFor(() => expect(lib.timeline).toHaveBeenCalledTimes(2));
  });

  it("makes one selected photo the cover through that photo's face of the person", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    vi.spyOn(api, "photoFaces").mockResolvedValue({
      items: [
        { id: "f-other", box: [0, 0, 0.1, 0.1], person: { id: "p9", name: null } },
        { id: "f-mine", box: [0.5, 0.5, 0.1, 0.1], person: { id: "p1", name: "Person A" } },
      ],
    });
    const update = vi.spyOn(api, "updatePerson").mockResolvedValue(person("p1", "Person A", 3));
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Select" }));
    const cover = screen.getByRole("button", { name: "Use as cover" });
    expect(cover).toBeDisabled();
    await userEvent.click(open(2));
    await userEvent.click(open(1));
    expect(cover).toBeDisabled();
    await userEvent.click(open(1));
    await userEvent.click(cover);
    await waitFor(() => expect(update).toHaveBeenCalledWith("p1", { cover_face_id: "f-mine" }));
    expect(api.photoFaces).toHaveBeenCalledWith("id-2");
  });

  it("merges into another person chosen from the list, never into themselves", async () => {
    vi.spyOn(api, "getPerson").mockImplementation(async (id) => person(id, id === "p1" ? "Person A" : null, 3));
    vi.spyOn(api, "listPeople").mockResolvedValue({ items: [person("p1", "Person A", 3), person("p2", "Person B", 5), person("p3", null, 1)] });
    const merge = vi.spyOn(api, "mergePerson").mockResolvedValue({ into: "p3", faces: 3 });
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Merge into…" }));
    const select = await screen.findByLabelText("Merge into");
    expect(within(select).queryByRole("option", { name: /Person A/ })).not.toBeInTheDocument();
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(2));
    await userEvent.selectOptions(select, "p3");
    await userEvent.click(screen.getByRole("button", { name: "Merge" }));
    await waitFor(() => expect(merge).toHaveBeenCalledWith("p1", "p3"));
    // The page for the person merged into opens (the old page's toast goes with it).
    await waitFor(() => expect(api.getPerson).toHaveBeenCalledWith("p3"));
  });

  it("deletes the person after asking, and goes back to People", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    const del = vi.spyOn(api, "deletePerson").mockResolvedValue(undefined);
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Delete person" }));
    const dialog = screen.getByRole("alertdialog", { name: "Delete Person A?" });
    expect(del).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete person" }));
    expect(await screen.findByText("all people")).toBeInTheDocument();
    expect(del).toHaveBeenCalledWith("p1");
  });
});

describe("'Is this X?'", () => {
  it("asks about each suggested face, with Yes and No named for the person", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    vi.spyOn(api, "personSuggestions").mockResolvedValueOnce({ items: [suggestion(1), suggestion(2)] }).mockResolvedValue({ items: [] });
    const assign = vi.spyOn(api, "assignFace").mockResolvedValue({ person: null });
    const reject = vi.spyOn(api, "rejectFace").mockResolvedValue(undefined);
    renderAt();
    expect(await screen.findByRole("heading", { name: "Is this Person A?" })).toBeInTheDocument();
    const section = screen.getByRole("region", { name: "Suggestions" });
    expect(section.querySelectorAll("img")).toHaveLength(2);
    await userEvent.click(within(section).getAllByRole("button", { name: "No, this is not Person A" })[1]!);
    await waitFor(() => expect(reject).toHaveBeenCalledWith("s2", "p1"));
    await userEvent.click(within(section).getByRole("button", { name: "Yes, this is Person A" }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("s1", { person_id: "p1" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Suggestions" })).not.toBeInTheDocument());
    // A yes changes the person's photos: the timeline is read again.
    await waitFor(() => expect(lib.timeline).toHaveBeenCalledTimes(2));
  });

  it("asks 'the same person?' of someone with no name, and shows nothing when there is nothing to ask", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", null, 3));
    vi.spyOn(api, "personSuggestions").mockResolvedValue({ items: [suggestion(1)] });
    renderAt();
    expect(await screen.findByRole("heading", { name: "Is this the same person?" })).toBeInTheDocument();
  });

  it("keeps the face in the list and says so when saving fails", async () => {
    vi.spyOn(api, "getPerson").mockResolvedValue(person("p1", "Person A", 3));
    vi.spyOn(api, "personSuggestions").mockResolvedValue({ items: [suggestion(1)] });
    vi.spyOn(api, "assignFace").mockRejectedValue(new Error("offline"));
    renderAt();
    await userEvent.click(await screen.findByRole("button", { name: "Yes, this is Person A" }));
    expect(await screen.findByText(/Couldn’t save your answer\. offline/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Yes, this is Person A" })).toBeInTheDocument();
  });
});
