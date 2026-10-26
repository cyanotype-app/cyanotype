import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { person } from "../test/fixtures";
import { AppShell } from "../shell/AppShell";
import { UploadPickerProvider } from "../shell/UploadPicker";
import { UploadProvider } from "../upload/UploadProvider";
import { PeopleProvider } from "../usePeople";
import { PeoplePage } from "./People";

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co", role: "member" });
});
afterEach(() => vi.restoreAllMocks());

const renderPeople = () =>
  render(
    <MemoryRouter initialEntries={["/people"]}>
      <AuthProvider>
        <PeopleProvider>
          <Routes>
            <Route path="/people" element={<PeoplePage />} />
            <Route path="/people/:id" element={<p>person page</p>} />
          </Routes>
        </PeopleProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

describe("the People page", () => {
  it("is not there while the feature is off: not found, and the list is never asked for", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: false });
    const list = vi.spyOn(api, "listPeople");
    renderPeople();
    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeInTheDocument();
    expect(list).not.toHaveBeenCalled();
  });

  it("shows round faces with names, counts, the unnamed in their own section, and hidden people in a collapsed one", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "listPeople").mockResolvedValue({
      items: [person("a", "Person A", 12), person("b", null, 1), person("c", "Person C", 2, { hidden: true }), person("d", "Person D", 0, { cover: null }), person("e", null, 4, { hidden: true })],
    });
    renderPeople();
    const grid = await screen.findByRole("list", { name: "People" });
    const a = within(grid).getByRole("link", { name: "Person A, 12 photos" });
    expect(a).toHaveAttribute("href", "/people/a");
    expect(within(a).getByText("Person A")).toBeInTheDocument();
    // The face is the preview, cropped by CSS (no other file).
    expect(a.querySelector("img")).toHaveAttribute("src", "https://s3.test/preview-a");
    expect(within(grid).getByRole("link", { name: "Person D, 0 photos" })).toBeInTheDocument();
    // Unnamed people are not in the named list: they have a section of their own, after it.
    expect(within(grid).queryByRole("link", { name: /Unnamed person/ })).not.toBeInTheDocument();
    const unnamed = screen.getByRole("region", { name: "Unnamed (1)" });
    const b = within(within(unnamed).getByRole("list", { name: "Unnamed people" })).getByRole("link", { name: "Unnamed person, 1 photo" });
    expect(within(b).getByText("Add a name")).toBeInTheDocument();
    expect(grid.compareDocumentPosition(unnamed) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Hidden people, named or not: in neither list, in a section that starts closed and comes last.
    expect(within(grid).queryByRole("link", { name: /Person C/ })).not.toBeInTheDocument();
    expect(within(unnamed).queryByRole("link", { name: /4 photos/ })).not.toBeInTheDocument();
    const hidden = screen.getByText("Hidden people (2)").closest("details")!;
    expect(hidden).not.toHaveAttribute("open");
    expect(unnamed.compareDocumentPosition(hidden) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await userEvent.click(screen.getByText("Hidden people (2)"));
    const hiddenList = screen.getByRole("list", { name: "Hidden people" });
    expect(within(hiddenList).getByRole("link", { name: "Person C, 2 photos" })).toBeInTheDocument();
    expect(within(hiddenList).getByRole("link", { name: "Unnamed person, 4 photos" })).toBeInTheDocument();
    expect(screen.getByText("3 people")).toBeInTheDocument();
  });

  it("shows a badge with the suggestions waiting on a person's card, named for assistive technology, and none at 0", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "listPeople").mockResolvedValue({
      items: [person("a", "Person A", 12, { pending: 3 }), person("b", "Person B", 5, { pending: 0 }), person("c", "Person C", 5)],
    });
    renderPeople();
    const a = await screen.findByRole("link", { name: "Person A, 12 photos, 3 photos to confirm" });
    expect(within(a).getByText("3")).toBeInTheDocument();
    for (const name of ["Person B, 5 photos", "Person C, 5 photos"]) {
      const card = screen.getByRole("link", { name });
      expect(card.querySelector(".person-card-badge")).toBeNull();
    }
  });

  describe("selecting people", () => {
    const list = [person("a", "Person A", 12), person("b", "Person B", 5), person("c", null, 2), person("d", "Person D", 1, { hidden: true })];
    const start = async () => {
      vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
      const listing = vi.spyOn(api, "listPeople").mockResolvedValue({ items: list });
      renderPeople();
      await screen.findByRole("list", { name: "People" });
      await userEvent.click(screen.getByRole("button", { name: "Select" }));
      return listing;
    };
    const pick = (name: RegExp) => userEvent.click(screen.getByRole("button", { name }));

    it("turns cards into toggles, counts them, and Escape clears and then leaves", async () => {
      await start();
      const bar = screen.getByRole("toolbar", { name: "Selection" });
      expect(within(bar).getByText("Select people")).toBeInTheDocument();
      expect(screen.queryByRole("link", { name: /Person A/ })).not.toBeInTheDocument();
      await pick(/^Person A/);
      await pick(/^Unnamed person/);
      expect(screen.getByRole("button", { name: /^Person A/ })).toHaveAttribute("aria-pressed", "true");
      expect(within(bar).getByText("2 selected")).toBeInTheDocument();
      await userEvent.click(within(bar).getByRole("button", { name: "Select all" }));
      expect(within(bar).getByText("4 selected")).toBeInTheDocument();
      await userEvent.keyboard("{Escape}");
      expect(within(bar).getByText("Select people")).toBeInTheDocument();
      await userEvent.keyboard("{Escape}");
      expect(screen.queryByRole("toolbar", { name: "Selection" })).not.toBeInTheDocument();
      expect(screen.getByRole("link", { name: /Person A/ })).toBeInTheDocument();
    });

    it("merges the selected people into the one chosen to stay, with an optional name, after confirming", async () => {
      const listing = await start();
      const bulk = vi.spyOn(api, "bulkPeople").mockResolvedValue({ done: 2, into: "b", faces: 3 });
      const bar = screen.getByRole("toolbar", { name: "Selection" });
      expect(within(bar).getByRole("button", { name: "Merge" })).toBeDisabled();
      await pick(/^Person A/);
      expect(within(bar).getByRole("button", { name: "Merge" })).toBeDisabled();
      await pick(/^Person B/);
      await pick(/^Unnamed person/);
      await userEvent.click(within(bar).getByRole("button", { name: "Merge" }));
      const dialog = screen.getByRole("dialog", { name: "Merge 3 people into one?" });
      // The named person with the most photos stays by default; the survivor can be chosen.
      expect(within(dialog).getByRole("radio", { name: /Person A/ })).toBeChecked();
      await userEvent.click(within(dialog).getByRole("radio", { name: /Person B/ }));
      await userEvent.type(within(dialog).getByLabelText("Name (optional)"), " Person N ");
      expect(bulk).not.toHaveBeenCalled();
      await userEvent.click(within(dialog).getByRole("button", { name: "Merge" }));
      await waitFor(() => expect(bulk).toHaveBeenCalledWith({ action: "merge", ids: ["a", "b", "c"], into: "b", name: "Person N" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      expect(screen.getByText("Merged 3 people into Person N")).toBeInTheDocument();
      // The list is read again and selection mode ends.
      expect(listing).toHaveBeenCalledTimes(2);
      expect(screen.queryByRole("toolbar", { name: "Selection" })).not.toBeInTheDocument();
    });

    it("shows a failed merge in the dialog and keeps the selection", async () => {
      await start();
      vi.spyOn(api, "bulkPeople").mockRejectedValue(new Error("boom"));
      await pick(/^Person A/);
      await pick(/^Person B/);
      await userEvent.click(screen.getByRole("button", { name: "Merge" }));
      const dialog = screen.getByRole("dialog");
      await userEvent.click(within(dialog).getByRole("button", { name: "Merge" }));
      expect(await within(dialog).findByRole("alert")).toHaveTextContent("boom");
      await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
      expect(screen.getByText("2 selected")).toBeInTheDocument();
    });

    it("hides and shows the selected people", async () => {
      await start();
      const bulk = vi.spyOn(api, "bulkPeople").mockResolvedValue({ done: 1 });
      const bar = screen.getByRole("toolbar", { name: "Selection" });
      // A shown person: Hide is offered, Show is not.
      await pick(/^Person A/);
      expect(within(bar).queryByRole("button", { name: "Show" })).not.toBeInTheDocument();
      await userEvent.click(within(bar).getByRole("button", { name: "Hide" }));
      await waitFor(() => expect(bulk).toHaveBeenCalledWith({ action: "hide", ids: ["a"] }));
      expect(await screen.findByText(/1 person hidden/)).toBeInTheDocument();
      // Selection mode ended; start again with a hidden person.
      await userEvent.click(screen.getByRole("button", { name: "Select" }));
      await pick(/^Person D/);
      expect(within(screen.getByRole("toolbar", { name: "Selection" })).queryByRole("button", { name: "Hide" })).not.toBeInTheDocument();
      await userEvent.click(within(screen.getByRole("toolbar", { name: "Selection" })).getByRole("button", { name: "Show" }));
      await waitFor(() => expect(bulk).toHaveBeenLastCalledWith({ action: "show", ids: ["d"] }));
    });

    it("deletes the selected people only after asking, and says the photos stay", async () => {
      await start();
      const bulk = vi.spyOn(api, "bulkPeople").mockResolvedValue({ done: 2 });
      await pick(/^Person B/);
      await pick(/^Unnamed person/);
      await userEvent.click(within(screen.getByRole("toolbar", { name: "Selection" })).getByRole("button", { name: "Delete" }));
      const dialog = screen.getByRole("dialog", { name: "Delete 2 people?" });
      expect(within(dialog).getByText(/The photos stay in your library/)).toBeInTheDocument();
      expect(bulk).not.toHaveBeenCalled();
      await userEvent.click(within(dialog).getByRole("button", { name: "Delete people" }));
      await waitFor(() => expect(bulk).toHaveBeenCalledWith({ action: "delete", ids: ["b", "c"] }));
      expect(await screen.findByText(/2 people deleted/)).toBeInTheDocument();
    });
  });

  it("explains an empty page and reports a failure", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
    const list = vi.spyOn(api, "listPeople").mockResolvedValueOnce({ items: [] });
    const { unmount } = renderPeople();
    expect(await screen.findByRole("heading", { name: "No people yet" })).toBeInTheDocument();
    unmount();
    list.mockRejectedValueOnce(new Error("boom"));
    renderPeople();
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
  });
});

describe("the way to People", () => {
  const renderShell = (status: "on" | "off" | "error") => {
    const s = vi.spyOn(api, "peopleStatus");
    if (status === "error") s.mockRejectedValue(new Error("no"));
    else s.mockResolvedValue({ enabled: status === "on" });
    render(
      <MemoryRouter>
        <AuthProvider>
          <PeopleProvider>
            <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
              <UploadPickerProvider>
                <AppShell>
                  <p>page</p>
                </AppShell>
              </UploadPickerProvider>
            </UploadProvider>
          </PeopleProvider>
        </AuthProvider>
      </MemoryRouter>,
    );
  };
  const library = () => within(screen.getByRole("navigation", { name: "Library" }));
  it("is in the sidebar next to Tags only when the feature is on", async () => {
    renderShell("on");
    await waitFor(() => expect(library().getByRole("link", { name: "People" })).toHaveAttribute("href", "/people"));
    expect(library().getByRole("link", { name: "Tags" })).toBeInTheDocument();
  });
  it.each(["off", "error"] as const)("is nowhere when the feature is %s", async (status) => {
    renderShell(status);
    expect(library().getByRole("link", { name: "Tags" })).toBeInTheDocument();
    await waitFor(() => expect(api.peopleStatus).toHaveBeenCalled());
    expect(screen.queryByRole("link", { name: "People" })).not.toBeInTheDocument();
  });
});
