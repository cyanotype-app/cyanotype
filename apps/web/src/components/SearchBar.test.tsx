import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { beforeEach, describe, expect, it } from "vitest";
import { SearchBar, SearchShortcut } from "./SearchBar";

function Where() {
  const l = useLocation();
  return <p data-testid="where">{l.pathname + l.search}</p>;
}
function Elsewhere() {
  const navigate = useNavigate();
  return (
    <button type="button" onClick={() => navigate("/search")}>
      go to search
    </button>
  );
}

const renderAt = (url: string, recentsInPage = false) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <SearchShortcut />
      <Routes>
        <Route path="/search" element={<SearchBar recentsInPage={recentsInPage}><button type="button">Filters</button></SearchBar>} />
        <Route path="*" element={<Elsewhere />} />
      </Routes>
      <Where />
    </MemoryRouter>,
  );

beforeEach(() => localStorage.clear());

describe("SearchBar", () => {
  it("is a labelled field with its children beside it", () => {
    renderAt("/search");
    expect(screen.getByRole("search")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Search photos" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Filters" })).toBeInTheDocument();
  });

  it("goes to the results with the query, and remembers it", async () => {
    renderAt("/search");
    await userEvent.type(screen.getByRole("combobox"), "dog on the beach{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent("/search?q=dog+on+the+beach");
    expect(JSON.parse(localStorage.getItem("cyanotype.recentSearches")!)).toEqual(["dog on the beach"]);
  });

  it("keeps the filters already chosen when the query changes", async () => {
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

  it("an empty query keeps the filters and drops the text", async () => {
    renderAt("/search?q=cat&favorite=true");
    await userEvent.clear(screen.getByRole("combobox"));
    await userEvent.type(screen.getByRole("combobox"), "{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent("/search?favorite=true");
  });

  it("clears the text with the x and keeps the focus in the field", async () => {
    renderAt("/search?q=cat");
    await userEvent.click(screen.getByRole("button", { name: "Clear search text" }));
    expect(screen.getByRole("combobox")).toHaveValue("");
    expect(screen.getByRole("combobox")).toHaveFocus();
    expect(screen.queryByRole("button", { name: "Clear search text" })).not.toBeInTheDocument();
  });

  it("drops down the recent searches that continue what is typed, runs one, and clears them", async () => {
    localStorage.setItem("cyanotype.recentSearches", JSON.stringify(["birthday cake", "red car", "red door"]));
    renderAt("/search?favorite=true");
    await userEvent.type(screen.getByRole("combobox"), "red");
    const list = await screen.findByRole("listbox", { name: "Recent searches" });
    expect(list).toHaveTextContent("red car");
    expect(list).toHaveTextContent("red door");
    expect(list).not.toHaveTextContent("birthday cake");
    await userEvent.click(screen.getByRole("button", { name: "red car" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/search?q=red+car&favorite=true");

    await userEvent.clear(screen.getByRole("combobox")); // the field shows the page's query until it is emptied
    await userEvent.click(await screen.findByRole("button", { name: "Clear recent searches" }));
    expect(localStorage.getItem("cyanotype.recentSearches")).toBeNull();
    expect(screen.queryByRole("button", { name: "red car" })).not.toBeInTheDocument();
  });

  it("does not drop the recent searches over a page that lists them itself", async () => {
    localStorage.setItem("cyanotype.recentSearches", JSON.stringify(["red car"]));
    renderAt("/search", true);
    await userEvent.click(screen.getByRole("combobox"));
    expect(screen.queryByRole("button", { name: "red car" })).not.toBeInTheDocument();
    await userEvent.type(screen.getByRole("combobox"), "re");
    expect(await screen.findByRole("button", { name: "red car" })).toBeInTheDocument();
  });

  it("works when the browser refuses storage", async () => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = () => {
      throw new Error("blocked");
    };
    try {
      renderAt("/search");
      await userEvent.type(screen.getByRole("combobox"), "dog{Enter}");
      expect(screen.getByTestId("where")).toHaveTextContent("/search?q=dog");
    } finally {
      Storage.prototype.getItem = real;
    }
  });

  it("takes focus when the search page opens with nothing searched, not when it opens with results", async () => {
    const empty = renderAt("/search");
    await waitFor(() => expect(screen.getByRole("combobox")).toHaveFocus());
    empty.unmount();
    renderAt("/search?q=cat");
    expect(screen.getByRole("combobox")).not.toHaveFocus();
  });
});

describe("the Ctrl+K shortcut", () => {
  it("opens the search page with the field focused, from any page", async () => {
    renderAt("/albums");
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    await userEvent.keyboard("{Control>}k{/Control}");
    expect(await screen.findByRole("combobox")).toHaveFocus();
    expect(screen.getByTestId("where")).toHaveTextContent("/search");
  });

  it("works with the Command key too, and on the search page keeps the results and selects the text", async () => {
    renderAt("/search?q=cat&favorite=true");
    await userEvent.click(document.body);
    await userEvent.keyboard("{Meta>}k{/Meta}");
    await waitFor(() => expect(screen.getByRole("combobox")).toHaveFocus());
    expect(screen.getByTestId("where")).toHaveTextContent("/search?q=cat&favorite=true");
    const input = screen.getByRole("combobox") as HTMLInputElement;
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 3]);
  });
});
