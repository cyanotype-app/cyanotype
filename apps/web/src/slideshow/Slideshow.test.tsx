import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "../api";
import type { Feed, SlideSource } from "./deck";
import { SETTINGS_KEY } from "./settings";
import { Slideshow } from "./Slideshow";
import { chunked, photos } from "./testKit";

beforeEach(() => {
  Object.defineProperty(HTMLImageElement.prototype, "decode", { configurable: true, value: () => Promise.resolve() });
  vi.spyOn(api, "previews").mockImplementation(async (ids) => ({
    items: ids.map((id) => ({ id, preview_url: `/img/${id}.jpg`, width: 4, height: 3, caption: null })),
  }));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
  delete (HTMLImageElement.prototype as { decode?: unknown }).decode;
  delete (document.documentElement as { requestFullscreen?: unknown }).requestFullscreen;
});

const photoSrc = () => document.querySelector<HTMLImageElement>(".slide-photo:last-of-type")?.getAttribute("src");
const open = (source: SlideSource = chunked(photos(5), 10).source, onClose = vi.fn()) => {
  render(<Slideshow source={source} onClose={onClose} />);
  return onClose;
};

describe("Slideshow", () => {
  it("shows the first photo with its place, and a labelled dialog", async () => {
    open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    expect(screen.getByRole("dialog", { name: "Slideshow: t" })).toBeInTheDocument();
    expect(screen.getByText("1 / 5")).toBeInTheDocument();
    // Previews, never originals: only the preview endpoint is asked.
    expect(api.previews).toHaveBeenCalled();
  });

  it("Space pauses and plays, and says so to a screen reader", async () => {
    const user = userEvent.setup();
    open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    expect(screen.getByRole("button", { name: "Pause" })).toBeInTheDocument();
    await user.keyboard(" ");
    expect(screen.getByRole("button", { name: "Play" })).toBeInTheDocument();
    expect(document.querySelector('[aria-live="polite"]')).toHaveTextContent("Slideshow paused");
    await user.keyboard(" ");
    expect(screen.getByRole("button", { name: "Pause" })).toBeInTheDocument();
    expect(document.querySelector('[aria-live="polite"]')).toHaveTextContent("Slideshow playing");
  });

  it("arrow keys and the buttons move between photos", async () => {
    const user = userEvent.setup();
    open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(photoSrc()).toBe("/img/p2.jpg"));
    expect(document.querySelector('[aria-live="polite"]')).toHaveTextContent("Photo 2 of 5");
    await user.keyboard("{ArrowLeft}");
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.click(screen.getByRole("button", { name: "Next photo" }));
    await waitFor(() => expect(photoSrc()).toBe("/img/p2.jpg"));
    await user.click(screen.getByRole("button", { name: "Previous photo" }));
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
  });

  it("Escape closes; with the settings open it closes those first", async () => {
    const user = userEvent.setup();
    const onClose = open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.click(screen.getByRole("button", { name: "Slideshow settings" }));
    expect(screen.getByRole("group", { name: "Slideshow settings" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("group", { name: "Slideshow settings" })).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("keeps its keys from the page underneath (the viewer's shortcuts)", async () => {
    const user = userEvent.setup();
    const under = vi.fn();
    document.addEventListener("keydown", under);
    open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.keyboard("{ArrowRight}s i{Escape}");
    document.removeEventListener("keydown", under);
    expect(under).not.toHaveBeenCalled();
  });

  it("remembers duration, order, transition and loop for this browser", async () => {
    const user = userEvent.setup();
    open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.click(screen.getByRole("button", { name: "Slideshow settings" }));
    await user.selectOptions(screen.getByLabelText("Time per photo"), "10");
    await user.selectOptions(screen.getByLabelText("Order"), "shuffle");
    await user.selectOptions(screen.getByLabelText("Transition"), "none");
    await user.click(screen.getByLabelText("Start again at the end"));
    expect(JSON.parse(localStorage.getItem(SETTINGS_KEY)!)).toEqual({ duration: 10, order: "shuffle", transition: "none", loop: false });
    // A new slideshow starts with them.
    document.body.innerHTML = "";
    open();
    await user.click(await screen.findByRole("button", { name: "Slideshow settings" }));
    expect(screen.getByLabelText("Time per photo")).toHaveValue("10");
    expect(screen.getByLabelText("Order")).toHaveValue("shuffle");
  });

  it("offers a fade only when the device does not ask for less motion", async () => {
    const user = userEvent.setup();
    vi.stubGlobal("matchMedia", (q: string) => ({
      matches: q.includes("reduce"),
      media: q,
      addEventListener() {},
      removeEventListener() {},
    }));
    open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.click(screen.getByRole("button", { name: "Slideshow settings" }));
    expect(screen.getByLabelText("Transition")).toBeDisabled();
    expect(screen.getByLabelText("Transition")).toHaveValue("none");
    expect(screen.getByText(/less motion/)).toBeInTheDocument();
  });

  it("has no full screen button where the browser cannot do it (an iPhone), and the viewer simply fills the window", async () => {
    open();
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    expect(screen.queryByRole("button", { name: /full screen/i })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog")).toHaveClass("slideshow");
  });

  it("has one where it can", async () => {
    Object.defineProperty(document.documentElement, "requestFullscreen", { configurable: true, value: vi.fn(() => Promise.resolve()) });
    open();
    expect(await screen.findByRole("button", { name: "Full screen" })).toBeInTheDocument();
  });

  it("says when there is nothing to show", async () => {
    open(chunked([], 10).source);
    expect(await screen.findByText("No photos to show")).toBeInTheDocument();
  });

  it("says the show is over, and plays again from the start", async () => {
    const user = userEvent.setup();
    localStorage.setItem(SETTINGS_KEY, JSON.stringify({ duration: 5, order: "listed", transition: "none", loop: false }));
    open(chunked(photos(1), 10).source);
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByText("That’s all")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Play again" }));
    await waitFor(() => expect(screen.queryByText("That’s all")).not.toBeInTheDocument());
    expect(photoSrc()).toBe("/img/p1.jpg");
  });

  it("asks to sign in again when the session ended", async () => {
    const dead: SlideSource = {
      title: "x",
      open: (): Feed => ({
        next: async () => {
          throw new ApiError(401, "Not signed in");
        },
      }),
    };
    open(dead);
    expect(await screen.findByRole("alert")).toHaveTextContent("Sign in again");
  });

  it("a share link that needs its password again says that instead", async () => {
    const dead: SlideSource = {
      title: "x",
      expired: { title: "This link is locked", text: "Open the link again." },
      open: (): Feed => ({
        next: async () => {
          throw new ApiError(401, "locked");
        },
      }),
    };
    open(dead);
    expect(await screen.findByText("This link is locked")).toBeInTheDocument();
  });

  it("tells the person when the server cannot be reached, and keeps the photo", async () => {
    let down = false;
    const failing: SlideSource = {
      title: "x",
      open: (): Feed => ({
        next: async () => {
          if (down) throw new ApiError(0, "Cannot reach the server.");
          return { items: photos(3), done: false };
        },
      }),
    };
    const user = userEvent.setup();
    open(failing);
    await waitFor(() => expect(photoSrc()).toBe("/img/p1.jpg"));
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(photoSrc()).toBe("/img/p2.jpg"));
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(photoSrc()).toBe("/img/p3.jpg"));
    // The chunk in memory is used up and the next one cannot be had.
    down = true;
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByText(/Can’t reach the server/)).toBeInTheDocument();
    expect(photoSrc()).toBe("/img/p3.jpg");
  });
});
