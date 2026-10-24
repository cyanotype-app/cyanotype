import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { LibraryTabs } from "../components/LibraryTabs";
import { person } from "../test/fixtures";
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

  it("shows round faces with names, 'Add a name' for the unnamed, counts, and hidden people in a collapsed section", async () => {
    vi.spyOn(api, "peopleStatus").mockResolvedValue({ enabled: true });
    vi.spyOn(api, "listPeople").mockResolvedValue({
      items: [person("a", "Person A", 12), person("b", null, 1), person("c", "Person C", 2, { hidden: true }), person("d", "Person D", 0, { cover: null })],
    });
    renderPeople();
    const grid = await screen.findByRole("list", { name: "People" });
    const a = within(grid).getByRole("link", { name: "Person A, 12 photos" });
    expect(a).toHaveAttribute("href", "/people/a");
    expect(within(a).getByText("Person A")).toBeInTheDocument();
    const b = within(grid).getByRole("link", { name: "Unnamed person, 1 photo" });
    expect(within(b).getByText("Add a name")).toBeInTheDocument();
    // The face is the preview, cropped by CSS (no other file).
    expect(a.querySelector("img")).toHaveAttribute("src", "https://s3.test/preview-a");
    expect(within(grid).getByRole("link", { name: "Person D, 0 photos" })).toBeInTheDocument();
    // Hidden people: not in the main list, in a section that starts closed.
    expect(within(grid).queryByRole("link", { name: /Person C/ })).not.toBeInTheDocument();
    const hidden = screen.getByText("Hidden people (1)").closest("details")!;
    expect(hidden).not.toHaveAttribute("open");
    await userEvent.click(screen.getByText("Hidden people (1)"));
    expect(within(screen.getByRole("list", { name: "Hidden people" })).getByRole("link", { name: "Person C, 2 photos" })).toBeInTheDocument();
    expect(screen.getByText("3 people")).toBeInTheDocument();
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
  const renderTabs = (status: "on" | "off" | "error") => {
    const s = vi.spyOn(api, "peopleStatus");
    if (status === "error") s.mockRejectedValue(new Error("no"));
    else s.mockResolvedValue({ enabled: status === "on" });
    render(
      <MemoryRouter>
        <AuthProvider>
          <PeopleProvider>
            <LibraryTabs />
          </PeopleProvider>
        </AuthProvider>
      </MemoryRouter>,
    );
  };
  it("is in the More menu next to Tags only when the feature is on", async () => {
    renderTabs("on");
    await userEvent.click(screen.getByLabelText("More views"));
    await waitFor(() => expect(screen.getByRole("link", { name: "People" })).toHaveAttribute("href", "/people"));
    expect(screen.getByRole("link", { name: "Tags" })).toBeInTheDocument();
  });
  it.each(["off", "error"] as const)("is nowhere when the feature is %s", async (status) => {
    renderTabs(status);
    await userEvent.click(screen.getByLabelText("More views"));
    expect(screen.getByRole("link", { name: "Tags" })).toBeInTheDocument();
    await waitFor(() => expect(api.peopleStatus).toHaveBeenCalled());
    expect(screen.queryByRole("link", { name: "People" })).not.toBeInTheDocument();
  });
});
