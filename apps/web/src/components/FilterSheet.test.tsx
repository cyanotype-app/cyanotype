import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Facets } from "../api";
import { album, person } from "../test/fixtures";
import { FilterSheet } from "./FilterSheet";

const facets: Facets = {
  total: 100,
  favorites: 7,
  with_location: 60,
  ratings: [
    { value: "5", count: 4 },
    { value: "3", count: 20 },
  ],
  types: [],
  cameras: [
    { value: "Canon EOS R5", count: 70 },
    { value: "Apple iPhone 15", count: 30 },
  ],
  lenses: [{ value: "RF 24-70mm F2.8 L", count: 55 }],
  places: [
    { value: "Portugal", kind: "country", count: 40 },
    { value: "Lisbon", kind: "city", count: 35 },
  ],
  years: [
    { value: "2024", count: 80 },
    { value: "2021", count: 20 },
  ],
};
const load = async () => ({ facets, albums: [album(1, { title: "Pets", photo_count: 12 })] });

describe("FilterSheet", () => {
  it("offers what the library has, with counts, and applies the choices together", async () => {
    const onApply = vi.fn();
    render(<FilterSheet value={{}} onApply={onApply} onClose={() => undefined} load={load} />);
    const dialog = screen.getByRole("dialog", { name: "Filters" });
    await waitFor(() => expect(within(dialog).getByRole("option", { name: "Canon EOS R5 (70)" })).toBeInTheDocument());

    await userEvent.selectOptions(screen.getByLabelText("Rating"), "4");
    await userEvent.selectOptions(screen.getByLabelText("Year"), "2024");
    await userEvent.selectOptions(screen.getByLabelText("Camera"), "Canon EOS R5");
    await userEvent.selectOptions(screen.getByLabelText("Lens"), "RF 24-70mm F2.8 L");
    await userEvent.selectOptions(screen.getByLabelText("Place"), "Lisbon");
    await userEvent.selectOptions(screen.getByLabelText("Location"), "true");
    await userEvent.selectOptions(screen.getByLabelText("Album"), "album-1");
    await userEvent.click(screen.getByRole("checkbox", { name: /Favorites only \(7\)/ }));
    await userEvent.click(screen.getByRole("radio", { name: "RAW" }));
    expect(onApply).not.toHaveBeenCalled(); // nothing is applied until "Show photos"
    await userEvent.click(screen.getByRole("button", { name: "Show photos" }));
    expect(onApply).toHaveBeenCalledWith({
      from: "2024-01-01",
      to: "2024-12-31",
      camera: "Canon EOS R5",
      lens: "RF 24-70mm F2.8 L",
      place: "Lisbon",
      has_location: true,
      album: "album-1",
      favorite: true,
      rating: 4,
      type: "raw",
    });
  });

  it("starts from the filters in force, keeps a value the library no longer lists, and clears everything", async () => {
    const onApply = vi.fn();
    render(
      <FilterSheet
        value={{ camera: "Old Camera", favorite: true, from: "2023-05-01", to: "2023-05-31" }}
        onApply={onApply}
        onClose={() => undefined}
        load={load}
      />,
    );
    await waitFor(() => expect(screen.getByRole("option", { name: "Canon EOS R5 (70)" })).toBeInTheDocument());
    expect(screen.getByLabelText("Camera")).toHaveValue("Old Camera");
    expect(screen.getByLabelText("From")).toHaveValue("2023-05-01");
    expect(screen.getByLabelText("Year")).toHaveValue(""); // a custom range is not a whole year
    await userEvent.click(screen.getByRole("button", { name: "Clear all" }));
    await userEvent.click(screen.getByRole("button", { name: "Show photos" }));
    expect(onApply).toHaveBeenCalledWith({});
  });

  it("closes on Escape and on Cancel, applying nothing", async () => {
    const onClose = vi.fn();
    const onApply = vi.fn();
    render(<FilterSheet value={{}} onApply={onApply} onClose={onClose} load={load} />);
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(onApply).not.toHaveBeenCalled();
  });

  it("says so when the options cannot be loaded, and still lets the user apply", async () => {
    render(
      <FilterSheet
        value={{}}
        onApply={() => undefined}
        onClose={() => undefined}
        load={() => Promise.reject(new Error("boom"))}
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
    expect(screen.getByRole("button", { name: "Show photos" })).toBeEnabled();
  });
});

describe("FilterSheet people", () => {
  const A = "6f1c2b0e-0000-4000-8000-000000000001";
  const B = "6f1c2b0e-0000-4000-8000-000000000002";
  const people = [person(A, "Person A", 12), person(B, "Person B", 5)];

  it("is not offered while People is off", async () => {
    render(<FilterSheet value={{}} onApply={() => undefined} onClose={() => undefined} load={load} />);
    await waitFor(() => expect(screen.getByRole("option", { name: "Canon EOS R5 (70)" })).toBeInTheDocument());
    expect(screen.queryByRole("group", { name: "People" })).not.toBeInTheDocument();
    expect(screen.queryByText(/Name a person/)).not.toBeInTheDocument();
  });

  it("chooses one or several people, who must all be in the photo, and applies them with the other filters", async () => {
    const onApply = vi.fn();
    render(<FilterSheet value={{ favorite: true }} onApply={onApply} onClose={() => undefined} load={load} people={people} />);
    const group = screen.getByRole("group", { name: "People" });
    expect(screen.getByText("Photos with all of the chosen people in them.")).toBeInTheDocument();
    await userEvent.click(within(group).getByRole("checkbox", { name: "Person A (12 photos)" }));
    await userEvent.click(within(group).getByRole("checkbox", { name: "Person B (5 photos)" }));
    await userEvent.selectOptions(screen.getByLabelText("Rating"), "4");
    await userEvent.click(screen.getByRole("button", { name: "Show photos" }));
    expect(onApply).toHaveBeenCalledWith({ favorite: true, rating: 4, people: `${A},${B}` });
  });

  it("starts from the people in force and clears them", async () => {
    const onApply = vi.fn();
    render(<FilterSheet value={{ people: B }} onApply={onApply} onClose={() => undefined} load={load} people={people} />);
    expect(screen.getByRole("checkbox", { name: "Person B (5 photos)" })).toBeChecked();
    await userEvent.click(screen.getByRole("checkbox", { name: "Person B (5 photos)" }));
    await userEvent.click(screen.getByRole("button", { name: "Show photos" }));
    expect(onApply).toHaveBeenCalledWith({});
  });

  it("says how to get people when none is named", () => {
    render(<FilterSheet value={{}} onApply={() => undefined} onClose={() => undefined} load={load} people={[]} />);
    expect(screen.getByText("Name a person on the People page to filter by them.")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "People" })).not.toBeInTheDocument();
  });
});
