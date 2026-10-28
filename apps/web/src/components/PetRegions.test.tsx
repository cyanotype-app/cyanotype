// The animals of a photo as regions over the picture: a link for a named pet, a button for any other, each with its name.
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PhotoPet } from "../api";
import { PetRegions } from "./PetRegions";

afterEach(cleanup);

const named: PhotoPet = { id: "d1", kind: "dog", box: [0.1, 0.2, 0.3, 0.4], score: 0.9, pet: { id: "p1", name: "Pet A" } };
const dog: PhotoPet = { id: "d2", kind: "dog", box: [0.5, 0.5, 0.2, 0.2], score: 0.9, pet: null };
const cat: PhotoPet = { id: "d3", kind: "cat", box: [0.6, 0.1, 0.2, 0.2], score: 0.9, pet: null };

function draw(showAll = false) {
  const onOpen = vi.fn();
  const view = render(<PetRegions animals={[named, dog, cat]} rect={{ left: 10, top: 20, width: 400, height: 300 }} showAll={showAll} onOpen={onOpen} />);
  return { onOpen, ...view };
}

describe("PetRegions", () => {
  it("places each box as fractions of the picture", () => {
    const { container } = draw();
    expect(container.querySelector(".face-regions")).toHaveStyle({ left: "10px", top: "20px", width: "400px", height: "300px" });
    expect(container.querySelector(".face-spot")).toHaveStyle({ left: "10%", top: "20%", width: "30%", height: "40%" });
  });
  it("labels a box with the pet's name, else Dog or Cat", () => {
    const { container } = draw();
    expect([...container.querySelectorAll(".region-label")].map((l) => l.textContent)).toEqual(["Pet A", "Dog", "Cat"]);
  });
  it("is a link for a named pet and a button for any other, each labelled and reachable with the keyboard", () => {
    draw();
    const link = screen.getByRole("link", { name: "Pet A: open their page" });
    expect(link).toHaveAttribute("tabindex", "0");
    expect(link).toHaveAttribute("data-pet-id", "d1");
    expect(link).toHaveAttribute("data-tap");
    expect(screen.getByRole("button", { name: "Dog: add to a pet" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cat: add to a pet" })).toBeInTheDocument();
  });
  it("opens with Enter, Space and an assistive technology's click, not a pointer's own click", async () => {
    const { onOpen } = draw();
    const region = screen.getByRole("button", { name: "Dog: add to a pet" });
    region.focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.keyboard(" ");
    expect(onOpen).toHaveBeenCalledTimes(2);
    expect(onOpen).toHaveBeenLastCalledWith(dog);
    fireEvent.click(region);
    expect(onOpen).toHaveBeenCalledTimes(3);
    fireEvent.click(region, { detail: 1 });
    expect(onOpen).toHaveBeenCalledTimes(3);
  });
  it("outlines every animal only when asked to", () => {
    const { container } = draw();
    expect(container.querySelector(".pet-regions")).not.toHaveClass("all");
    cleanup();
    expect(draw(true).container.querySelector(".pet-regions")).toHaveClass("all");
  });
});
