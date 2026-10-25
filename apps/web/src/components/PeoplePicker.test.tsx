import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { person } from "../test/fixtures";
import { PeoplePicker } from "./PeoplePicker";

const people = [person("a", "Person A", 12), person("b", "Person B", 1), person("c", "Someone Else", 4)];

function Harness({ start = [] as string[], onChange = vi.fn(), max = 10 }: { start?: string[]; onChange?: (ids: string[]) => void; max?: number }) {
  const [ids, setIds] = useState(start);
  return (
    <PeoplePicker
      label="People"
      people={people}
      value={ids}
      max={max}
      onChange={(next) => {
        setIds(next);
        onChange(next);
      }}
    />
  );
}

describe("PeoplePicker", () => {
  it("is a named group of checkboxes with each person's photo count", async () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    expect(screen.getByRole("group", { name: "People" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: "Person A (12 photos)" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Person B (1 photo)" }));
    expect(onChange).toHaveBeenLastCalledWith(["a", "b"]);
    await userEvent.click(screen.getByRole("checkbox", { name: "Person A (12 photos)" }));
    expect(onChange).toHaveBeenLastCalledWith(["b"]);
  });

  it("narrows the list by name and keeps the chosen people in view", async () => {
    render(<Harness start={["c"]} />);
    await userEvent.type(screen.getByRole("searchbox", { name: "Find a person" }), "person");
    expect(screen.getAllByRole("checkbox")).toHaveLength(3);
    expect(screen.getByRole("checkbox", { name: /Someone Else/ })).toBeChecked();
    await userEvent.clear(screen.getByRole("searchbox", { name: "Find a person" }));
    await userEvent.type(screen.getByRole("searchbox", { name: "Find a person" }), "zzz");
    expect(screen.getByText("No one is called that.")).toBeInTheDocument();
  });

  it("stops at the most people allowed, and can take off a person who is gone", async () => {
    const onChange = vi.fn();
    render(<Harness start={["a", "gone"]} max={2} onChange={onChange} />);
    expect(screen.getByRole("checkbox", { name: "Person B (1 photo)" })).toBeDisabled();
    await userEvent.click(screen.getByRole("checkbox", { name: "Unknown person" }));
    expect(onChange).toHaveBeenLastCalledWith(["a"]);
    expect(screen.getByRole("checkbox", { name: "Person B (1 photo)" })).toBeEnabled();
  });
});
