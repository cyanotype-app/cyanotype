import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it } from "vitest";
import { LibraryTabs } from "./LibraryTabs";

function Where() {
  const l = useLocation();
  return <p data-testid="where">{l.pathname + l.search}</p>;
}

const renderAt = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <LibraryTabs />
      <Routes>
        <Route path="*" element={<Where />} />
      </Routes>
    </MemoryRouter>,
  );

beforeEach(() => localStorage.clear());

describe("SearchBar in the library header", () => {
  it("is a field and an icon next to the three tabs, and adds no fourth tab", () => {
    renderAt("/library");
    expect(screen.getAllByRole("link")).toHaveLength(3);
    expect(screen.getByRole("combobox", { name: "Search photos" })).toBeInTheDocument();
    const icon = screen.getByRole("button", { name: "Search photos" });
    expect(icon).toHaveAttribute("aria-expanded", "false");
    expect(icon.closest(".search")).toHaveAttribute("data-open", "false");
  });

  it("expands from the icon, and the header then hides the tabs (phones)", async () => {
    renderAt("/library");
    await userEvent.click(screen.getByRole("button", { name: "Search photos" }));
    expect(screen.getByRole("button", { name: "Search photos", expanded: true })).toBeInTheDocument();
    expect(document.querySelector(".library-header")).toHaveAttribute("data-searching", "true");
    await waitFor(() => expect(screen.getByRole("combobox")).toHaveFocus());
    await userEvent.click(screen.getByRole("button", { name: "Close search" }));
    expect(document.querySelector(".library-header")).toHaveAttribute("data-searching", "false");
  });

  it("goes to the results page with the query, and remembers it", async () => {
    renderAt("/library");
    await userEvent.type(screen.getByRole("combobox"), "dog on the beach{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent("/search?q=dog+on+the+beach");
    expect(JSON.parse(localStorage.getItem("cyanotype.recentSearches")!)).toEqual(["dog on the beach"]);
  });

  it("keeps the filters already chosen on the search page when the query changes", async () => {
    renderAt("/search?q=cat&favorite=true&type=raw");
    const input = screen.getByRole("combobox");
    expect(input).toHaveValue("cat");
    await userEvent.clear(input);
    await userEvent.type(input, "dog{Enter}");
    const where = screen.getByTestId("where").textContent!;
    expect(where).toContain("q=dog");
    expect(where).toContain("favorite=true");
    expect(where).toContain("type=raw");
  });

  it("an empty query on the search page keeps the filters and drops the text", async () => {
    renderAt("/search?q=cat&favorite=true");
    await userEvent.clear(screen.getByRole("combobox"));
    await userEvent.type(screen.getByRole("combobox"), "{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent("/search?favorite=true");
  });

  it("offers recent searches on focus, runs one when chosen, and clears them", async () => {
    localStorage.setItem("cyanotype.recentSearches", JSON.stringify(["birthday cake", "red car"]));
    renderAt("/library");
    await userEvent.click(screen.getByRole("combobox"));
    const list = await screen.findByRole("listbox", { name: "Recent searches" });
    expect(list).toHaveTextContent("birthday cake");
    await userEvent.click(screen.getByRole("button", { name: "red car" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/search?q=red+car");

    await userEvent.clear(screen.getByRole("combobox")); // the field shows the page's query until it is emptied
    await userEvent.click(await screen.findByRole("button", { name: "Clear recent searches" }));
    expect(localStorage.getItem("cyanotype.recentSearches")).toBeNull();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });
});
