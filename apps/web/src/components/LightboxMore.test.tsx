import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { detail, item } from "../test/fixtures";
import { Lightbox } from "./Lightbox";

const items = [1, 2, 3].map((n) => item(n));
type Handlers = { onAddToAlbum: () => void; onShare: () => void; onArchive: () => void; onHide: () => void; onTrash: () => void };

function Harness({ list = items, onClose = () => {}, ...handlers }: Partial<Handlers> & { list?: typeof items; onClose?: () => void }) {
  const [i, setI] = useState(0);
  return <Lightbox items={list} index={i} onIndexChange={setI} onClose={onClose} loadDetail={async (id) => detail(Number(id.split("-")[1]))} {...handlers} />;
}
const all = () => ({ onAddToAlbum: vi.fn(), onShare: vi.fn(), onArchive: vi.fn(), onHide: vi.fn(), onTrash: vi.fn() });
const more = () => screen.getByRole("button", { name: "More" });
const menuLabels = () => screen.getAllByRole("menuitem").map((m) => m.textContent);

describe("Lightbox More menu", () => {
  it("is a menu button that lists the actions that apply", async () => {
    const user = userEvent.setup();
    render(<Harness {...all()} />);
    expect(more()).toHaveAttribute("aria-haspopup", "menu");
    expect(more()).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await user.click(more());
    expect(more()).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("menu")).toHaveAttribute("id", more().getAttribute("aria-controls"));
    expect(menuLabels()).toEqual(["Add to album", "Archive", "Hide", "Move to trash"]);
    expect(screen.getByRole("menuitem", { name: "Add to album" })).toHaveFocus();
  });

  it("says Restore and Unhide for an archived, hidden photo", async () => {
    const user = userEvent.setup();
    render(<Harness list={[item(1, { archived: true, hidden: true })]} {...all()} />);
    await user.click(more());
    expect(menuLabels()).toEqual(["Add to album", "Restore", "Unhide", "Move to trash"]);
  });

  it("only lists the actions whose handlers are given", async () => {
    const user = userEvent.setup();
    render(<Harness onTrash={vi.fn()} onHide={vi.fn()} />);
    await user.click(more());
    expect(menuLabels()).toEqual(["Hide", "Move to trash"]);
  });

  it("is not rendered when nothing overflows (Share and Download stay in the bar)", () => {
    render(<Harness onShare={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "More" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Share" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Download" })).toBeInTheDocument();
  });

  it("moves focus with the arrow keys, Home and End", async () => {
    const user = userEvent.setup();
    render(<Harness {...all()} />);
    await user.click(more());
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Archive" })).toHaveFocus();
    await user.keyboard("{End}");
    expect(screen.getByRole("menuitem", { name: "Move to trash" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Add to album" })).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("menuitem", { name: "Move to trash" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("menuitem", { name: "Add to album" })).toHaveFocus();
  });

  it("closes on Escape, gives focus back to More and leaves the lightbox open", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Harness {...all()} onClose={onClose} />);
    await user.click(more());
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(more()).toHaveFocus();
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("closes when you click or tap anywhere else", async () => {
    const user = userEvent.setup();
    render(<Harness {...all()} />);
    await user.click(more());
    await user.click(screen.getByText("1 / 3"));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("runs the chosen action with the photo and closes", async () => {
    const user = userEvent.setup();
    const h = all();
    render(<Harness {...h} />);
    await user.click(more());
    await user.click(screen.getByRole("menuitem", { name: "Archive" }));
    expect(h.onArchive).toHaveBeenCalledExactlyOnceWith(items[0]);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(more()).toHaveFocus();

    await user.click(more());
    await user.keyboard("{End}{Enter}");
    expect(h.onTrash).toHaveBeenCalledExactlyOnceWith(items[0]);
    expect(h.onAddToAlbum).not.toHaveBeenCalled();
    expect(h.onHide).not.toHaveBeenCalled();
  });

  it("closes when the photo changes", async () => {
    const user = userEvent.setup();
    render(<Harness {...all()} />);
    await user.click(more());
    await user.click(screen.getByRole("button", { name: "Next photo" }));
    expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(more()).toHaveAttribute("aria-expanded", "false");
  });

  it("closes the file chooser when it opens, and the other way round", async () => {
    const user = userEvent.setup();
    render(<Harness list={[item(1, { edited: true })]} {...all()} />);
    const download = screen.getByRole("button", { name: "Download" });
    await user.click(download);
    expect(download).toHaveAttribute("aria-expanded", "true");
    await user.click(more());
    expect(download).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("menu")).toBeInTheDocument();
    await user.click(download);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(download).toHaveAttribute("aria-expanded", "true");
  });
});
