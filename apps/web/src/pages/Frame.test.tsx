import { act, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "../api";
import { AuthProvider } from "../auth";
import { FRAME_KEY } from "../frameConfig";
import { photos } from "../slideshow/testKit";
import { Frame } from "./Frame";

const ALBUM = "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d";

/** Where the router is, for tests of what Frame navigates to. */
function Where() {
  const l = useLocation();
  return <p data-testid="where">{l.pathname + l.search}</p>;
}

function renderFrame(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider>
        <Routes>
          <Route path="/frame" element={<Frame />} />
          <Route path="/login" element={<Where />} />
          <Route path="/library" element={<Where />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

/** A library of `n` photos the server hands out 4 at a time; `p<k>` previews are `/img/p<k>.jpg`. */
function serve(n = 12, tweak: (i: number) => object = () => ({})) {
  const all = photos(n).map((p, i) => ({
    ...p,
    original_name: `${p.id}.jpg`,
    content_type: "image/jpeg",
    size_bytes: 1,
    width: 4,
    height: 3,
    thumbhash: null,
    thumb_url: null,
    ...tweak(i),
  }));
  const page = vi.spyOn(api, "slideshowPage").mockImplementation(async (_scope, opts) => {
    const at = opts.cursor ? Number(opts.cursor) : 0;
    const items = all.slice(at, at + 4);
    return { items: items as never, next_cursor: at + 4 < n ? String(at + 4) : null, total: n };
  });
  const previews = vi.spyOn(api, "previews").mockImplementation(async (ids) => ({
    items: ids.map((id) => ({ id, preview_url: `/img/${id}.jpg`, width: 4, height: 3, caption: id === "p1" ? "A day at the lake" : null })),
  }));
  return { page, previews };
}

const shown = () => Array.from(document.querySelectorAll<HTMLImageElement>(".slide-photo")).map((i) => i.getAttribute("src"));
const flush = async (ms = 0) => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
};

const created = vi.fn();
const revoked = vi.fn();
beforeEach(() => {
  vi.useFakeTimers();
  Object.defineProperty(HTMLImageElement.prototype, "decode", { configurable: true, value: () => Promise.resolve() });
  Object.defineProperty(URL, "createObjectURL", { configurable: true, value: created });
  Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: revoked });
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
  created.mockClear();
  revoked.mockClear();
  delete (HTMLImageElement.prototype as { decode?: unknown }).decode;
});

describe("session", () => {
  it("shows a clear Sign in again screen when the session has ended, never a blank page", async () => {
    vi.spyOn(api, "me").mockRejectedValue(new ApiError(401, "Not signed in"));
    renderFrame("/frame?source=library");
    await flush();
    expect(screen.getByRole("heading", { name: "Sign in again" })).toBeInTheDocument();
    const link = screen.getByRole("link", { name: "Sign in" });
    expect(link).toHaveAttribute("href", "/login");
  });

  it("sends the person back to the frame after signing in", async () => {
    vi.spyOn(api, "me").mockRejectedValue(new ApiError(401, "Not signed in"));
    renderFrame("/frame?source=library&interval=60");
    await flush();
    // (The login page reads `from` from the link's state.)
    expect(screen.getByRole("link", { name: "Sign in" })).toBeInTheDocument();
  });

  it("waits for the network after a reboot instead of claiming to be signed out", async () => {
    const me = vi.spyOn(api, "me").mockRejectedValue(new ApiError(0, "Cannot reach the server."));
    serve();
    renderFrame("/frame?source=library");
    await flush();
    expect(screen.getByRole("heading", { name: "Waiting for the network" })).toBeInTheDocument();
    expect(screen.queryByText("Sign in again")).not.toBeInTheDocument();
    me.mockResolvedValue({ id: "u", email: "a@b.co" });
    await flush(10_000);
    await flush();
    expect(shown().at(-1)).toBe("/img/p1.jpg");
  });

  it("the session ending in the middle of a run gives the same screen", async () => {
    const { page } = serve();
    renderFrame("/frame?source=library&interval=5");
    await flush();
    expect(shown().at(-1)).toBe("/img/p1.jpg");
    page.mockRejectedValue(new ApiError(401, "Not signed in"));
    vi.mocked(api.previews).mockRejectedValue(new ApiError(401, "Not signed in"));
    vi.mocked(api.me).mockRejectedValue(new ApiError(401, "Not signed in"));
    await flush(5 * 60_000);
    expect(screen.getByRole("heading", { name: "Sign in again" })).toBeInTheDocument();
  });
});

describe("setup", () => {
  it("asks what to show when the address says nothing, and starts the frame with the answer", async () => {
    serve();
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [{ id: ALBUM, title: "Lake trip" } as never] });
    renderFrame("/frame");
    await flush();
    expect(screen.getByRole("heading", { name: "Photo frame" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start the frame" })).toBeEnabled();
    fireEvent.click(screen.getByLabelText("One album"));
    expect(screen.getByRole("button", { name: "Start the frame" })).toBeDisabled();
    await flush();
    fireEvent.change(screen.getByLabelText("Album"), { target: { value: ALBUM } });
    fireEvent.change(screen.getByLabelText("Change the photo every"), { target: { value: "60" } });
    fireEvent.click(screen.getByLabelText("Show the time and date"));
    fireEvent.click(screen.getByRole("button", { name: "Start the frame" }));
    await flush();
    expect(vi.mocked(api.slideshowPage).mock.calls[0]![0]).toEqual({ album: ALBUM });
    expect(vi.mocked(api.slideshowPage).mock.calls[0]![1]).toMatchObject({ order: "shuffle" });
    expect(shown().at(-1)).toBe("/img/p1.jpg");
    expect(document.querySelector(".frame-clock")).toBeInTheDocument();
    expect(localStorage.getItem(FRAME_KEY)).toContain(`album=${ALBUM}`);
  });

  it("a browser that remembers the choice goes straight to the frame", async () => {
    serve();
    localStorage.setItem(FRAME_KEY, "source=favorites&interval=10");
    renderFrame("/frame");
    await flush();
    expect(vi.mocked(api.slideshowPage).mock.calls[0]![0]).toBe("favorites");
    expect(shown().at(-1)).toBe("/img/p1.jpg");
  });

  it("?setup opens the setup screen again, with the current choice", async () => {
    serve();
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    renderFrame("/frame?setup&source=favorites");
    await flush();
    expect(screen.getByRole("button", { name: "Start the frame" })).toBeInTheDocument();
  });
});

describe("the frame", () => {
  it("shows random photos from the whole library, one after another, on the interval", async () => {
    const { page } = serve(12);
    renderFrame("/frame?source=library&interval=10");
    await flush();
    expect(page.mock.calls[0]![0]).toBe("library");
    expect(page.mock.calls[0]![1]).toMatchObject({ order: "shuffle", limit: 40 });
    expect(document.querySelector(".slide-stage.blur")).toBeInTheDocument();
    // The picture also fills the background, blurred (same address: no second download).
    expect(document.querySelectorAll('img[src="/img/p1.jpg"]')).toHaveLength(2);
    const first = shown().at(-1);
    await flush(9999);
    expect(shown().at(-1)).toBe(first);
    await flush(1);
    expect(shown().at(-1)).not.toBe(first);
  });

  it("never shows an archived photo, and a hidden or trashed one never reaches it (the server leaves them out)", async () => {
    serve(12, (i) => ({ archived: i % 2 === 0 }));
    renderFrame("/frame?album=" + ALBUM + "&interval=5");
    await flush();
    const seen = new Set<string>();
    for (let i = 0; i < 40; i++) {
      for (const s of shown()) seen.add(s!);
      await flush(5000);
    }
    expect(seen.size).toBeGreaterThan(0);
    // p1, p3, p5... are the archived ones: only the even-numbered photos may be shown.
    for (const s of seen) expect(Number(/p(\d+)/.exec(s)![1]) % 2, `${s} is archived`).toBe(0);
  });

  it("keeps the page small over days: at most two photos (and their blurred copies) in the page, no object URLs, no growth", async () => {
    serve(12);
    renderFrame("/frame?source=library&interval=5");
    await flush();
    let most = 0;
    const baseline = document.body.querySelectorAll("*").length;
    for (let i = 0; i < 600; i++) {
      await flush(5000);
      most = Math.max(most, document.querySelectorAll("img").length);
    }
    expect(most).toBeLessThanOrEqual(4);
    expect(document.body.querySelectorAll("*").length).toBeLessThanOrEqual(baseline + 8);
    // The CSP has no blob: in img-src, so the frame never makes object URLs (and so has none to revoke).
    expect(created).not.toHaveBeenCalled();
    expect(created.mock.calls.length).toBe(revoked.mock.calls.length);
  });

  it("keeps showing the last photo when the network drops, says so, and carries on when it is back", async () => {
    const { page, previews } = serve(40);
    renderFrame("/frame?source=library&interval=5");
    await flush();
    await flush(5000); // second photo (the third is loaded ahead)
    const last = shown().at(-1);
    page.mockRejectedValue(new ApiError(0, "Cannot reach the server."));
    previews.mockRejectedValue(new ApiError(0, "Cannot reach the server."));
    // Long enough to run out of what was loaded ahead and then of the chunk in memory.
    for (let i = 0; i < 20; i++) await flush(5000);
    expect(shown().at(-1)).toBeTruthy();
    expect(document.querySelectorAll(".slide-photo").length).toBeLessThanOrEqual(2);
    expect(screen.getAllByText("Offline. Trying again…").length).toBeGreaterThan(0);
    expect(screen.queryByRole("heading", { name: "Sign in again" })).not.toBeInTheDocument();
    const stuck = shown().at(-1);
    // Back online: the retry (waits grow to a few minutes) finds the server and the show goes on.
    page.mockImplementation(async (_s, o) => ({ items: photos(4, "n") as never, next_cursor: o.cursor ? null : "4", total: 8 }));
    previews.mockImplementation(async (ids) => ({
      items: ids.map((id) => ({ id, preview_url: `/img/${id}.jpg`, width: 4, height: 3, caption: null })),
    }));
    window.dispatchEvent(new Event("online"));
    await flush();
    await flush(5000);
    await flush(5000);
    expect(screen.queryByText("Offline. Trying again…")).not.toBeInTheDocument();
    expect(shown().at(-1)).not.toBe(stuck);
    expect(last).toBeTruthy();
  });

  it("the clock and the caption show only when asked for", async () => {
    serve();
    const { unmount } = renderFrame("/frame?source=library&interval=5");
    await flush();
    expect(document.querySelector(".frame-clock")).not.toBeInTheDocument();
    expect(document.querySelector(".frame-caption")).not.toBeInTheDocument();
    unmount();
    serve();
    renderFrame("/frame?source=library&interval=5&clock=1&caption=1");
    await flush();
    expect(document.querySelector(".frame-clock time")).toBeInTheDocument();
    expect(screen.getByText("A day at the lake")).toBeInTheDocument();
  });

  it("says there is nothing to show for an empty library", async () => {
    vi.spyOn(api, "slideshowPage").mockResolvedValue({ items: [], next_cursor: null, total: 0 });
    renderFrame("/frame?source=library");
    await flush();
    expect(screen.getByRole("heading", { name: "No photos to show" })).toBeInTheDocument();
  });
});

describe("the tap menu", () => {
  it("opens on a tap, closes by itself, and leads out", async () => {
    serve();
    renderFrame("/frame?source=library&interval=3600");
    await flush();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(document.querySelector(".frame")!);
    expect(screen.getByRole("dialog", { name: "Photo frame menu" })).toBeInTheDocument();
    await flush(10_001);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(document.querySelector(".frame")!);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(document.querySelector(".frame")!);
    fireEvent.click(screen.getByRole("button", { name: "Exit the frame" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/library");
  });

  it("Change what is shown opens the setup", async () => {
    serve();
    vi.spyOn(api, "listAlbums").mockResolvedValue({ items: [] });
    renderFrame("/frame?source=library&interval=3600");
    await flush();
    fireEvent.click(document.querySelector(".frame")!);
    fireEvent.click(screen.getByRole("button", { name: "Change what is shown" }));
    await flush();
    expect(screen.getByRole("button", { name: "Start the frame" })).toBeInTheDocument();
  });

  it("Escape opens and closes it for a keyboard", async () => {
    serve();
    renderFrame("/frame?source=library&interval=3600");
    await flush();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
