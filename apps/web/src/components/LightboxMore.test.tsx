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

describe("Lightbox menu", () => {
  it("is a menu button that lists the secondary actions that apply", async () => {
    const user = userEvent.setup();
    render(<Harness {...all()} />);
    expect(more()).toHaveAttribute("aria-haspopup", "menu");
    expect(more()).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await user.click(more());
    expect(more()).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("menu")).toBeInTheDocument();
    // Share and Move to trash are buttons of the bar, not menu items.
    expect(menuLabels()).toEqual(["Add to album", "Download", "Archive", "Hide"]);
  });

  it("says Restore and Unhide for an archived, hidden photo", async () => {
    const user = userEvent.setup();
    render(<Harness list={[item(1, { archived: true, hidden: true })]} {...all()} />);
    await user.click(more());
    expect(menuLabels()).toEqual(["Add to album", "Download", "Restore", "Unhide"]);
  });

  it("only lists the actions whose handlers are given", async () => {
    const user = userEvent.setup();
    render(<Harness onTrash={vi.fn()} onHide={vi.fn()} />);
    await user.click(more());
    expect(menuLabels()).toEqual(["Download", "Hide"]);
  });

  it("keeps Share and Move to trash as buttons of the bar", () => {
    render(<Harness onShare={vi.fn()} onTrash={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Share" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Move to trash" })).toBeInTheDocument();
  });

  it("moves focus with the arrow keys, Home and End", async () => {
    const user = userEvent.setup();
    render(<Harness {...all()} />);
    await user.click(more());
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Download" })).toHaveFocus();
    await user.keyboard("{End}");
    expect(screen.getByRole("menuitem", { name: "Hide" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("menuitem", { name: "Add to album" })).toHaveFocus();
  });

  it("closes on Escape, gives focus back to More and leaves the lightbox open", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Harness {...all()} onClose={onClose} />);
    await user.click(more());
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    expect(more()).toHaveFocus();
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("runs the chosen action with the photo and closes", async () => {
    const user = userEvent.setup();
    const h = all();
    render(<Harness {...h} />);
    await user.click(more());
    await user.click(screen.getByRole("menuitem", { name: "Archive" }));
    expect(h.onArchive).toHaveBeenCalledExactlyOnceWith(items[0]);
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    await user.click(more());
    await user.keyboard("{End}{Enter}");
    expect(h.onHide).toHaveBeenCalledExactlyOnceWith(items[0]);
    expect(h.onAddToAlbum).not.toHaveBeenCalled();
    expect(h.onTrash).not.toHaveBeenCalled();
  });

  it("does not take the viewer's shortcuts while the menu has the keyboard", async () => {
    const user = userEvent.setup();
    const onTrash = vi.fn();
    render(<Harness {...all()} onTrash={onTrash} />);
    await user.click(more());
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("dialog", { name: "photo-1.jpg" })).toBeInTheDocument();
    expect(onTrash).not.toHaveBeenCalled();
  });
});
