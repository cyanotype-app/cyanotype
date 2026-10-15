import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
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
