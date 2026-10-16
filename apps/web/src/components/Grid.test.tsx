import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { item, withRaw } from "../test/fixtures";
import { Grid } from "./Grid";

describe("Grid", () => {
  it("badges paired tiles with RAW and only those", () => {
    render(<Grid items={[withRaw(1), item(2), item(3, { paired: [] })]} onOpen={() => {}} onThumbError={() => {}} />);
    const tiles = screen.getAllByRole("button");
    expect(tiles[0]).toHaveTextContent("RAW");
    expect(tiles[1]).not.toHaveTextContent("RAW");
    expect(tiles[2]).not.toHaveTextContent("RAW");
    expect(screen.getAllByText("RAW")).toHaveLength(1);
  });
});

describe("Grid favorites", () => {
  it("shows no heart unless the page handles favorites", () => {
    render(<Grid items={[item(1, { favorite: true })]} onOpen={() => {}} onThumbError={() => {}} />);
    expect(screen.queryByRole("button", { name: /favorites/ })).not.toBeInTheDocument();
  });

  it("hearts a tile without opening it, and only marks favorites while selecting", () => {
    const onFavorite = vi.fn();
    const onOpen = vi.fn();
    const items = [item(1), item(2, { favorite: true })];
    const { rerender } = render(<Grid items={items} onOpen={onOpen} onThumbError={() => {}} onFavorite={onFavorite} />);
    fireEvent.click(screen.getByRole("button", { name: "Add photo-1.jpg to favorites" }));
    expect(onFavorite).toHaveBeenCalledWith("id-1", true);
    fireEvent.click(screen.getByRole("button", { name: "Remove photo-2.jpg from favorites" }));
    expect(onFavorite).toHaveBeenLastCalledWith("id-2", false);
    expect(onOpen).not.toHaveBeenCalled();

    rerender(<Grid items={items} onOpen={onOpen} onThumbError={() => {}} onFavorite={onFavorite} selecting />);
    expect(screen.queryByRole("button", { name: /favorites/ })).not.toBeInTheDocument();
    expect(document.querySelectorAll(".tile-heart.on")).toHaveLength(1);
  });
});
