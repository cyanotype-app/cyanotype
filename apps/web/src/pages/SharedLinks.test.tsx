import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { shareApi } from "../shareApi";
import { link } from "../test/shareFixtures";
import { SharedLinks } from "./SharedLinks";

let writeText: ReturnType<typeof vi.fn>;
/** userEvent.setup() installs its own clipboard, so look at what the page wrote through whatever is current. */
function watchClipboard(result: "ok" | "denied" = "ok") {
  writeText = vi.fn().mockImplementation(async () => {
    if (result === "denied") throw new Error("denied");
  });
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
}
beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  watchClipboard();
});
afterEach(() => vi.restoreAllMocks());

const renderPage = () =>
  render(
    <MemoryRouter>
      <AuthProvider>
        <SharedLinks />
      </AuthProvider>
    </MemoryRouter>,
  );

const rows = () => within(screen.getByRole("list", { name: "Shared links" })).getAllByRole("listitem");

describe("SharedLinks", () => {
  it("lists each link with what it shares, its state and its address", async () => {
    vi.spyOn(shareApi, "list").mockResolvedValue({
      items: [
        link(1, { kind: "album", album: { id: "a", title: "Summer" }, item_count: 12, view_count: 5, has_password: true }),
        link(2, { title: "For Sam", expired: true, expires_at: "2020-01-01T00:00:00Z", include_originals: true }),
        link(3, { allow_download: false }),
      ],
    });
    renderPage();
    await screen.findByRole("list", { name: "Shared links" });
    const [a, b, c] = rows();
    expect(within(a!).getByText("Summer")).toBeInTheDocument();
    expect(within(a!).getByText("Password")).toBeInTheDocument();
    expect(within(a!).getByText(/Album · 12 photos · Never expires · 5 views/)).toBeInTheDocument();
    expect(within(a!).getByText("Downloads: JPEG without location")).toBeInTheDocument();
    expect(within(a!).getByLabelText("Link for Summer")).toHaveValue(`${window.location.origin}/s/${"tok1".padEnd(32, "x")}`);
    expect(within(b!).getByText("For Sam")).toBeInTheDocument();
    expect(within(b!).getByText("Expired")).toBeInTheDocument();
    expect(within(b!).getByText("Originals")).toBeInTheDocument();
    expect(within(b!).getByText("Downloads: originals, with location")).toBeInTheDocument();
    expect(within(c!).getByText("No downloads")).toBeInTheDocument();
    expect(within(c!).getByRole("link", { name: "Open" })).toHaveAttribute("href", `${window.location.origin}/s/${"tok3".padEnd(32, "x")}`);
  });

  it("explains the empty state", async () => {
    vi.spyOn(shareApi, "list").mockResolvedValue({ items: [] });
    renderPage();
    expect(await screen.findByText("No shared links yet")).toBeInTheDocument();
  });

  it("copies a link", async () => {
    vi.spyOn(shareApi, "list").mockResolvedValue({ items: [link(1)] });
    const user = userEvent.setup();
    watchClipboard();
    renderPage();
    await screen.findByRole("list", { name: "Shared links" });
    await user.click(screen.getByRole("button", { name: "Copy link" }));
    expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/s/${"tok1".padEnd(32, "x")}`);
    expect(await screen.findByText("Link copied")).toBeInTheDocument();
  });

  it("revokes after confirmation and the row disappears", async () => {
    vi.spyOn(shareApi, "list").mockResolvedValue({ items: [link(1, { title: "One" }), link(2, { title: "Two" })] });
    const revoke = vi.spyOn(shareApi, "revoke").mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("list", { name: "Shared links" });
    await user.click(within(rows()[0]!).getByRole("button", { name: "Revoke" }));
    expect(revoke).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Revoke link" }));
    expect(revoke).toHaveBeenCalledWith("share-1");
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(screen.getByText("Two")).toBeInTheDocument();
    expect(screen.getByText(/no longer works/)).toBeInTheDocument();
  });

  it("keeps the link when the confirmation is cancelled", async () => {
    vi.spyOn(shareApi, "list").mockResolvedValue({ items: [link(1)] });
    const revoke = vi.spyOn(shareApi, "revoke").mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("list", { name: "Shared links" });
    await user.click(screen.getByRole("button", { name: "Revoke" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(revoke).not.toHaveBeenCalled();
    expect(rows()).toHaveLength(1);
  });

  it("edits a link and shows the saved state", async () => {
    vi.spyOn(shareApi, "list").mockResolvedValue({ items: [link(1, { title: "Old" })] });
    vi.spyOn(shareApi, "update").mockResolvedValue(link(1, { title: "New", has_password: true }));
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("list", { name: "Shared links" });
    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.clear(screen.getByLabelText("Title (optional)"));
    await user.type(screen.getByLabelText("Title (optional)"), "New");
    await user.type(screen.getByLabelText("Password (optional)"), "abcd1234");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Link updated")).toBeInTheDocument();
    expect(within(rows()[0]!).getByText("New")).toBeInTheDocument();
    expect(within(rows()[0]!).getByText("Password")).toBeInTheDocument();
  });

  it("shows a load error", async () => {
    vi.spyOn(shareApi, "list").mockRejectedValue(new Error("boom"));
    renderPage();
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
  });
});
