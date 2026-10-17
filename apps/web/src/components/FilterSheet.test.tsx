import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Facets } from "../api";
import { album } from "../test/fixtures";
import { FilterSheet } from "./FilterSheet";

const facets: Facets = {
  total: 100,
  favorites: 7,
  with_location: 60,
  types: [],
  cameras: [
    { value: "Canon EOS R5", count: 70 },
    { value: "Apple iPhone 15", count: 30 },
  ],
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

    await userEvent.selectOptions(screen.getByLabelText("Year"), "2024");
    await userEvent.selectOptions(screen.getByLabelText("Camera"), "Canon EOS R5");
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
      place: "Lisbon",
      has_location: true,
      album: "album-1",
      favorite: true,
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
