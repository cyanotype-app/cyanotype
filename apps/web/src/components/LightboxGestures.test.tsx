// The viewer's finger gestures: drag sideways to page (the neighbours are there, previews, under the finger), drag down to close,
// and the bars that go away when nothing is touched and come back on a tap. (The motion itself is checked in a browser:
// e2e/viewer-gesture-layout.spec.ts.)
import { createEvent, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { detail, item } from "../test/fixtures";
import { Lightbox } from "./Lightbox";

const items = [1, 2, 3].map((n) => item(n));

function Harness({ start = 1, onClose = () => {} }: { start?: number; onClose?: () => void }) {
  const [i, setI] = useState(start);
  return <Lightbox items={items} index={i} onIndexChange={setI} onClose={onClose} loadDetail={async (id) => detail(Number(id.split("-")[1]))} />;
}

beforeEach(() => {
  Object.defineProperty(window, "visualViewport", { configurable: true, value: { scale: 1 } });
});

const stage = () => document.querySelector(".viewer-stage")!;

/** One finger from (x0, y0) to (x1, y1): a few moves, over `ms`. */
function drag(el: Element, [x0, y0]: [number, number], [x1, y1]: [number, number], { ms = 200, type = "touch" } = {}) {
  const t0 = performance.now();
  // (jsdom stamps events with the real time and ignores one given: set it on the event, as a browser's would be.)
  const send = (kind: "pointerDown" | "pointerMove" | "pointerUp", f: number) => {
    const ev = createEvent[kind](el, { pointerType: type, pointerId: 1, clientX: x0 + (x1 - x0) * f, clientY: y0 + (y1 - y0) * f });
    Object.defineProperty(ev, "timeStamp", { value: t0 + ms * f });
    fireEvent(el, ev);
  };
  send("pointerDown", 0);
  for (const f of [0.25, 0.5, 0.75, 1]) send("pointerMove", f);
  send("pointerUp", 1);
}

describe("the paging strip", () => {
  it("has the neighbours' previews beside the photo, and none past the ends", async () => {
    const { unmount } = render(<Harness start={1} />);
    await waitFor(() => expect(screen.getByAltText("photo-2.jpg")).toHaveAttribute("src", "https://s3.test/preview-2"));
    // Previous and next are drawn (hidden from assistive technology: they are not the photo on show).
    await waitFor(() => expect(document.querySelectorAll(".viewer-slot")).toHaveLength(3));
    await waitFor(() => expect(Array.from(document.querySelectorAll(".viewer-slot[aria-hidden='true'] img")).map((i) => i.getAttribute("src"))).toEqual(["https://s3.test/preview-1", "https://s3.test/preview-3"]));
    unmount();
    render(<Harness start={0} />);
    expect(document.querySelectorAll(".viewer-slot")).toHaveLength(2);
  });

  it("a long drag left settles on the next photo, right on the previous one", async () => {
    render(<Harness start={1} />);
    drag(stage(), [600, 300], [150, 300]);
    await waitFor(() => expect(screen.getByRole("dialog", { name: "photo-3.jpg" })).toBeInTheDocument(), { timeout: 4000 });
    drag(stage(), [100, 300], [700, 300]);
    await waitFor(() => expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument(), { timeout: 4000 });
  });

  it("a short, slow drag springs back to the same photo", async () => {
    render(<Harness start={1} />);
    drag(stage(), [300, 300], [260, 300], { ms: 900 });
    await new Promise((r) => setTimeout(r, 600));
    expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();
  });

  it("does not go past the first or the last photo, and a mouse does not page", async () => {
    render(<Harness start={0} />);
    drag(stage(), [100, 300], [800, 300]);
    drag(stage(), [800, 300], [100, 300], { type: "mouse" });
    await new Promise((r) => setTimeout(r, 600));
    expect(screen.getByRole("dialog", { name: "photo-1.jpg" })).toBeInTheDocument();
  });

  it("paging is off for a second finger", async () => {
    render(<Harness start={1} />);
    fireEvent.pointerDown(stage(), { pointerType: "touch", pointerId: 2, clientX: 200, clientY: 500 });
    drag(stage(), [600, 300], [100, 300]);
    fireEvent.pointerUp(stage(), { pointerType: "touch", pointerId: 2 });
    await new Promise((r) => setTimeout(r, 600));
    expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();
  });
});

describe("drag down", () => {
  it("closes the viewer on a long drag, and leaves it on a short slow one", async () => {
    const onClose = vi.fn();
    render(<Harness start={1} onClose={onClose} />);
    drag(stage(), [200, 200], [210, 240], { ms: 900 });
    await new Promise((r) => setTimeout(r, 300));
    expect(onClose).not.toHaveBeenCalled();
    drag(stage(), [200, 200], [210, 500]);
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce(), { timeout: 3000 });
  });
});
