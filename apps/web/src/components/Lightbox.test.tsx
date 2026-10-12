import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { detail, item } from "../test/fixtures";
import { Lightbox } from "./Lightbox";

const items = [1, 2, 3].map((n) => item(n));

function Harness({ start = 0, onClose = () => {} }: { start?: number; onClose?: () => void }) {
  const [i, setI] = useState(start);
  const loadDetail = async (id: string) => detail(Number(id.split("-")[1]));
  return <Lightbox items={items} index={i} onIndexChange={setI} onClose={onClose} loadDetail={loadDetail} />;
}

describe("Lightbox", () => {
  it("shows the preview of the current photo and moves with the arrow keys", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const dialog = screen.getByRole("dialog", { name: "photo-1.jpg" });
    expect(dialog).toBeInTheDocument();
    await waitFor(() => expect(screen.getByAltText("photo-1.jpg")).toHaveAttribute("src", "https://s3.test/preview-1"));

    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByAltText("photo-2.jpg")).toHaveAttribute("src", "https://s3.test/preview-2"));

    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("dialog", { name: "photo-1.jpg" })).toBeInTheDocument();
  });

  it("stops at both ends instead of wrapping", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("dialog", { name: "photo-1.jpg" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous photo" })).toBeDisabled();
    await user.keyboard("{ArrowRight}{ArrowRight}{ArrowRight}");
    expect(screen.getByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next photo" })).toBeDisabled();
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("falls back to the thumbnail while the detail request fails", async () => {
    render(
      <Lightbox
        items={items}
        index={0}
        onIndexChange={() => {}}
        onClose={() => {}}
        loadDetail={() => Promise.reject(new Error("boom"))}
      />,
    );
    await waitFor(() => expect(screen.getByText("Could not load this photo.")).toBeInTheDocument());
    expect(screen.getByAltText("photo-1.jpg")).toHaveAttribute("src", "https://s3.test/thumb-1");
  });
});
