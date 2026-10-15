import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { detail, item, withRaw } from "../test/fixtures";
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

  it("offers Move to trash (button and Delete key) for the current photo only when wired up", async () => {
    const user = userEvent.setup();
    const onTrash = vi.fn();
    const { rerender } = render(
      <Lightbox items={items} index={1} onIndexChange={() => {}} onClose={() => {}} loadDetail={async () => detail(2)} onTrash={onTrash} />,
    );
    await user.click(screen.getByRole("button", { name: "Move to trash" }));
    expect(onTrash).toHaveBeenLastCalledWith(items[1]);
    await user.keyboard("{Delete}");
    expect(onTrash).toHaveBeenCalledTimes(2);

    rerender(<Lightbox items={items} index={1} onIndexChange={() => {}} onClose={() => {}} loadDetail={async () => detail(2)} />);
    expect(screen.queryByRole("button", { name: "Move to trash" })).not.toBeInTheDocument();
    await user.keyboard("{Delete}");
    expect(onTrash).toHaveBeenCalledTimes(2);
  });

  describe("RAW + JPEG pairs", () => {
    it("shows both files with their sizes and an Unpair action", async () => {
      const user = userEvent.setup();
      const onUnpair = vi.fn();
      const paired = [withRaw(1), item(2)];
      render(
        <Lightbox
          items={paired}
          index={0}
          onIndexChange={() => {}}
          onClose={() => {}}
          loadDetail={async () => detail(1)}
          onUnpair={onUnpair}
        />,
      );
      expect(screen.getByText("RAW + JPEG")).toBeInTheDocument();
      expect(screen.getByText("photo-1.CR3")).toBeInTheDocument();
      expect(screen.getByText("24 MB")).toBeInTheDocument();
      expect(screen.getByText("1001 B")).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Unpair" }));
      expect(onUnpair).toHaveBeenCalledWith(paired[0]);
    });

    it("leaves single photos alone", () => {
      render(
        <Lightbox items={[item(1)]} index={0} onIndexChange={() => {}} onClose={() => {}} loadDetail={async () => detail(1)} onUnpair={() => {}} />,
      );
      expect(screen.queryByText(/RAW \+/)).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Unpair" })).not.toBeInTheDocument();
    });

    it("names the pair after a HEIC primary and hides Unpair when it is not wired up", async () => {
      const heic = item(1, { content_type: "image/heic", original_name: "photo-1.HEIC", paired: withRaw(1).paired });
      render(<Lightbox items={[heic]} index={0} onIndexChange={() => {}} onClose={() => {}} loadDetail={async () => detail(1)} />);
      expect(screen.getByText("RAW + HEIC")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Unpair" })).not.toBeInTheDocument();
    });
  });
});
