import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type Tag } from "../api";
import { AuthProvider } from "../auth";
import { TagsPage } from "../pages/Tags";
import { TagDialog } from "./TagDialog";
import { TagInput } from "./TagInput";
import { useState } from "react";

afterEach(() => vi.restoreAllMocks());

const tags: Tag[] = [
  { id: "t1", name: "Beach", count: 5 },
  { id: "t2", name: "Barbecue", count: 9 },
];
const load = async (typed: string) => tags.filter((t) => t.name.toLowerCase().includes(typed.toLowerCase()));

function Harness({ existingOnly = false, start = [] as string[] }: { existingOnly?: boolean; start?: string[] }) {
  const [v, setV] = useState<readonly string[]>(start);
  return (
    <>
      <TagInput value={v} onChange={setV} label="Add tags" existingOnly={existingOnly} load={load} />
      <output aria-label="chosen">{v.join("|")}</output>
    </>
  );
}

describe("TagInput", () => {
  it("adds typed names with Enter or a comma, picks a suggestion, takes the last back with Backspace", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const field = screen.getByRole("combobox", { name: "Add tags" });
    await user.type(field, "sun set{Enter}");
    await user.type(field, "trip/2024, beach");
    await user.keyboard("{Enter}");
    expect(screen.getByLabelText("chosen")).toHaveTextContent("sun set|trip/2024|Beach"); // the library's spelling wins
    await user.type(field, "bar");
    const option = await screen.findByRole("option", { name: /Barbecue/ });
    await user.click(option);
    expect(screen.getByLabelText("chosen")).toHaveTextContent("sun set|trip/2024|Beach|Barbecue");
    await user.keyboard("{Backspace}");
    expect(screen.getByLabelText("chosen")).toHaveTextContent("sun set|trip/2024|Beach");
    await user.click(screen.getByRole("button", { name: "Remove trip/2024" }));
    expect(screen.getByLabelText("chosen")).toHaveTextContent("sun set|Beach");
  });

  it("refuses a bad name and, for existing tags only, a name that is not one", async () => {
    const user = userEvent.setup();
    render(<Harness existingOnly />);
    const field = screen.getByRole("combobox", { name: "Add tags" });
    await user.type(field, "nothing here{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("not one of your tags");
    await user.clear(field);
    await user.type(field, "beach{Enter}");
    await waitFor(() => expect(screen.getByLabelText("chosen")).toHaveTextContent("Beach"));
  });

  it("leaves no timer behind when it goes away right after a blur", () => {
    vi.useFakeTimers();
    try {
      const { unmount } = render(<Harness />);
      const field = screen.getByRole("combobox", { name: "Add tags" });
      fireEvent.focus(field);
      fireEvent.blur(field);
      unmount();
      expect(vi.getTimerCount()).toBe(0); // the list-closing timer would fire after teardown
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not let a name longer than 100 characters in", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.type(screen.getByRole("combobox"), `${"x".repeat(101)}{Enter}`);
    expect(await screen.findByRole("alert")).toHaveTextContent("at most 100");
    expect(screen.getByLabelText("chosen")).toHaveTextContent("");
  });
});

describe("TagDialog", () => {
  it("sends the names to add and remove for the selection and reports the result", async () => {
    const user = userEvent.setup();
    const apply = vi.spyOn(api, "applyTags").mockResolvedValue({ added: 4, removed: 1 });
    vi.spyOn(api, "listTags").mockResolvedValue({ items: tags });
    const onDone = vi.fn();
    render(<TagDialog target={["a", "b"]} count={2} onClose={() => {}} onDone={onDone} />);
    expect(screen.getByRole("dialog", { name: "Tag 2 photos" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
    await user.type(screen.getByRole("combobox", { name: "Add tags" }), "new one, other{Enter}");
    await user.type(screen.getByRole("combobox", { name: "Remove tags" }), "beach{Enter}");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    expect(apply).toHaveBeenCalledWith(["a", "b"], { add: ["new one", "other"], remove: ["Beach"] });
    await waitFor(() => expect(onDone).toHaveBeenCalledWith({ added: 4, removed: 1, message: "Added new one and other, removed Beach on 2 photos" }));
  });

  it("shows a failure and keeps the dialog open", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "listTags").mockResolvedValue({ items: [] });
    vi.spyOn(api, "applyTags").mockRejectedValue(new Error("boom"));
    render(<TagDialog target={["a"]} count={1} allowRemove={false} onClose={() => {}} onDone={() => {}} />);
    expect(screen.queryByRole("combobox", { name: "Remove tags" })).not.toBeInTheDocument();
    await user.type(screen.getByRole("combobox", { name: "Add tags" }), "x{Enter}");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
  });
});

describe("TagsPage", () => {
  it("lists tags with counts, filters by name and orders on the server", async () => {
    const user = userEvent.setup();
    const list = vi.spyOn(api, "listTags").mockResolvedValue({ items: tags });
    render(
      <MemoryRouter>
        <AuthProvider>
          <Routes>
            <Route path="/" element={<TagsPage />} />
          </Routes>
        </AuthProvider>
      </MemoryRouter>,
    );
    expect(await screen.findByRole("link", { name: /Beach/ })).toHaveAttribute("href", "/tags/t1");
    expect(screen.getByText("9 photos")).toBeInTheDocument();
    await user.type(screen.getByRole("searchbox", { name: "Find a tag" }), "barb");
    expect(screen.queryByRole("link", { name: /Beach/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Most used" }));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith({ sort: "count", limit: 5000 }));
  });

  it("explains an empty list", async () => {
    vi.spyOn(api, "listTags").mockResolvedValue({ items: [] });
    render(
      <MemoryRouter>
        <AuthProvider>
          <TagsPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    expect(await screen.findByText("No tags yet")).toBeInTheDocument();
  });
});
