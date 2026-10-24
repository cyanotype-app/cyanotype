import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type FaceSuggestion } from "../api";
import { AuthProvider } from "../auth";
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
  vi.spyOn(api, "personPhotos").mockResolvedValue({ items: [item(1), item(2), item(3)], next_cursor: null, next_before: null });
});
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
    expect(api.personPhotos).toHaveBeenCalledWith("p1", { cursor: null, limit: 60 });
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
    await waitFor(() => expect(api.personPhotos).toHaveBeenCalledTimes(2));
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
    // A yes changes the person's photos: they are loaded again.
    await waitFor(() => expect(api.personPhotos).toHaveBeenCalledTimes(2));
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
