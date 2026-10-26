import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { SlideshowProvider } from "../slideshow/SlideshowHost";
import { detail, item } from "../test/fixtures";
import { Lightbox } from "./Lightbox";

const items = [1, 2, 3, 4].map((n) => item(n));

function Harness({ start = 0 }: { start?: number }) {
  const [i, setI] = useState(start);
  return <Lightbox items={items} index={i} onIndexChange={setI} onClose={() => {}} loadDetail={async (id) => detail(Number(id.split("-")[1]))} />;
}

beforeEach(() => {
  Object.defineProperty(HTMLImageElement.prototype, "decode", { configurable: true, value: () => Promise.resolve() });
  vi.spyOn(api, "previews").mockImplementation(async (ids) => ({
    items: ids.map((id) => ({ id, preview_url: `/img/${id}.jpg`, width: 4, height: 3, caption: null })),
  }));
});
afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  delete (HTMLImageElement.prototype as { decode?: unknown }).decode;
});

const shown = () => document.querySelector<HTMLImageElement>(".slide-photo")?.getAttribute("src");

describe("Play from here", () => {
  it("is not offered where there is no slideshow to start", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: "More" }));
    expect(screen.queryByRole("menuitem", { name: "Play from here" })).not.toBeInTheDocument();
  });

  it("the menu item starts a slideshow at the photo on show, and Escape returns to the viewer", async () => {
    const user = userEvent.setup();
    render(
      <SlideshowProvider>
        <Harness start={2} />
      </SlideshowProvider>,
    );
    await user.click(screen.getByRole("button", { name: "More" }));
    await user.click(screen.getByRole("menuitem", { name: "Play from here" }));
    await waitFor(() => expect(shown()).toBe("/img/id-3.jpg"));
    expect(screen.getByRole("dialog", { name: "Slideshow: Photos" })).toBeInTheDocument();
    // The viewer's own keys do not act while the slideshow is open: the arrow moves the slideshow, not the viewer.
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(shown()).toBe("/img/id-4.jpg"));
    expect(screen.getByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: /Slideshow/ })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument();
  });

  it("the S key does the same", async () => {
    const user = userEvent.setup();
    render(
      <SlideshowProvider>
        <Harness start={1} />
      </SlideshowProvider>,
    );
    await user.keyboard("s");
    await waitFor(() => expect(shown()).toBe("/img/id-2.jpg"));
  });
});
