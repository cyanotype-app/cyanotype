import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AnimalPicker } from "./AnimalPicker";

const candidates = [
  { detection_id: "d1", box: [0.1, 0.2, 0.3, 0.4] as [number, number, number, number] },
  { detection_id: "d2", box: [0.55, 0.5, 0.25, 0.3] as [number, number, number, number] },
];

describe("AnimalPicker", () => {
  it("outlines each animal where its box is, numbered, and says which is chosen", () => {
    render(<AnimalPicker url="https://s3.test/p" candidates={candidates} value="d2" onChange={vi.fn()} />);
    expect(screen.getByAltText("The photo, with each animal outlined")).toHaveAttribute("src", "https://s3.test/p");
    const one = screen.getByRole("button", { name: "Animal 1 of 2" });
    const two = screen.getByRole("button", { name: "Animal 2 of 2" });
    expect(one).toHaveStyle({ left: "10%", top: "20%", width: "30%", height: "40%" });
    expect(two).toHaveStyle({ left: "55%", top: "50%", width: "25%", height: "30%" });
    expect(one).toHaveAttribute("aria-pressed", "false");
    expect(two).toHaveAttribute("aria-pressed", "true");
  });

  it("tells the caller which animal was tapped", async () => {
    const onChange = vi.fn();
    render(<AnimalPicker url="https://s3.test/p" candidates={candidates} value={null} onChange={onChange} />);
    await userEvent.click(screen.getByRole("button", { name: "Animal 1 of 2" }));
    expect(onChange).toHaveBeenCalledWith("d1");
  });
});
