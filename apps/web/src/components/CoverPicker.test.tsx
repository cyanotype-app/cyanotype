import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type CoverFace } from "../api";
import { person } from "../test/fixtures";
import { CoverPicker } from "./CoverPicker";

afterEach(() => vi.restoreAllMocks());

const faces = (from: number, to: number): CoverFace[] =>
  Array.from({ length: to - from + 1 }, (_, i) => ({ face_id: `cf${from + i}`, asset_id: `a${from + i}`, box: [0.1, 0.1, 0.2, 0.2], preview_url: `https://s3.test/c${from + i}` }));

describe("CoverPicker", () => {
  it("lists the faces as numbered buttons, marks the current cover, and chooses one", async () => {
    vi.spyOn(api, "personCoverFaces").mockResolvedValue({ items: faces(1, 3), next_cursor: null });
    const choose = vi.fn().mockResolvedValue(undefined);
    render(<CoverPicker person={person("p1", "Person A", 3, { cover: { face_id: "cf2", asset_id: "a2", box: [0, 0, 1, 1], preview_url: "https://s3.test/c2" } })} onChoose={choose} onCancel={vi.fn()} />);
    const dialog = await screen.findByRole("dialog", { name: "Change cover of Person A" });
    const grid = await within(dialog).findByRole("list", { name: "Faces" });
    expect(within(grid).getAllByRole("button")).toHaveLength(3);
    expect(within(grid).getByRole("button", { name: "Face 2, the current cover" })).toHaveAttribute("aria-pressed", "true");
    expect(within(dialog).queryByRole("button", { name: "Show more" })).not.toBeInTheDocument();
    await userEvent.click(within(grid).getByRole("button", { name: "Face 3" }));
    expect(choose).toHaveBeenCalledWith("cf3");
  });

  it("pages: 'Show more' adds the next page and puts the focus on its first face", async () => {
    const page = vi
      .spyOn(api, "personCoverFaces")
      .mockResolvedValueOnce({ items: faces(1, 2), next_cursor: "2" })
      .mockResolvedValueOnce({ items: faces(3, 4), next_cursor: null });
    render(<CoverPicker person={person("p1", "Person A")} onChoose={vi.fn()} onCancel={vi.fn()} />);
    await userEvent.click(await screen.findByRole("button", { name: "Show more" }));
    await waitFor(() => expect(screen.getAllByRole("button", { name: /^Face / })).toHaveLength(4));
    expect(page).toHaveBeenLastCalledWith("p1", { cursor: "2", limit: 24 });
    expect(screen.getByRole("button", { name: "Face 3" })).toHaveFocus();
    expect(screen.queryByRole("button", { name: "Show more" })).not.toBeInTheDocument();
  });

  it("closes with Escape and Cancel, keeps the focus inside, and says so when there are no faces or loading fails", async () => {
    vi.spyOn(api, "personCoverFaces").mockResolvedValueOnce({ items: [], next_cursor: null });
    const cancel = vi.fn();
    const { unmount } = render(<CoverPicker person={person("p1", "Person A")} onChoose={vi.fn()} onCancel={cancel} />);
    expect(await screen.findByText("There are no faces to choose from yet.")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(cancel).toHaveBeenCalledTimes(1);
    // Tab wraps around inside the dialog.
    await userEvent.tab();
    expect(screen.getByRole("dialog")).toContainElement(document.activeElement as HTMLElement);
    unmount();
    vi.spyOn(api, "personCoverFaces").mockRejectedValueOnce(new Error("boom"));
    render(<CoverPicker person={person("p1", "Person A")} onChoose={vi.fn()} onCancel={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
  });

  it("shows a failure to save and lets the owner choose again", async () => {
    vi.spyOn(api, "personCoverFaces").mockResolvedValue({ items: faces(1, 2), next_cursor: null });
    const choose = vi.fn().mockRejectedValueOnce(new Error("nope")).mockResolvedValueOnce(undefined);
    render(<CoverPicker person={person("p1", "Person A")} onChoose={choose} onCancel={vi.fn()} />);
    await userEvent.click(await screen.findByRole("button", { name: "Face 1" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("nope");
    await userEvent.click(screen.getByRole("button", { name: "Face 1" }));
    expect(choose).toHaveBeenCalledTimes(2);
  });
});
