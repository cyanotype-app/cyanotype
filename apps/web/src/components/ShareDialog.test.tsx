import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { shareApi } from "../shareApi";
import { link } from "../test/shareFixtures";
import { EditShareDialog, ShareDialog, type ShareTarget } from "./ShareDialog";

let writeText: ReturnType<typeof vi.fn>;
/** userEvent.setup() installs its own clipboard, so look at what the page wrote through whatever is current. */
function watchClipboard(result: "ok" | "denied" = "ok") {
  writeText = vi.fn().mockImplementation(async () => {
    if (result === "denied") throw new Error("denied");
  });
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
}
beforeEach(() => {
  watchClipboard();
});
afterEach(() => vi.restoreAllMocks());

const renderCreate = (target: ShareTarget, onClose = vi.fn()) =>
  render(
    <MemoryRouter>
      <ShareDialog target={target} onClose={onClose} />
    </MemoryRouter>,
  );

describe("ShareDialog (create)", () => {
  it("shares the selected photos with the defaults: no expiry, downloads on, originals off", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1, { token: "T".repeat(32) }));
    const user = userEvent.setup();
    watchClipboard();
    renderCreate({ kind: "photos", ids: ["a", "b"] });
    expect(screen.getByRole("heading", { name: "Share 2 photos" })).toBeInTheDocument();
    expect(screen.getByLabelText("Allow downloads")).toBeChecked();
    expect(screen.getByLabelText(/Include originals/)).not.toBeChecked();
    expect(screen.getByText(/JPEG copy without location/)).toBeInTheDocument();
    expect(screen.queryByText(/keep their GPS location/)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(create).toHaveBeenCalledWith({
      asset_ids: ["a", "b"],
      title: undefined,
      expires_at: null,
      password: undefined,
      allow_download: true,
      include_originals: false,
    });
    // The address is shown, selected for copying, and already on the clipboard.
    const url = await screen.findByLabelText("Share link");
    expect(url).toHaveValue(`${window.location.origin}/s/${"T".repeat(32)}`);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/s/${"T".repeat(32)}`));
    expect(screen.getByText(/Copied to the clipboard/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Shared links" })).toHaveAttribute("href", "/shared");
  });

  it("shows the link as a QR code too, to open it on a phone", async () => {
    vi.spyOn(shareApi, "create").mockResolvedValue(link(1, { token: "Q".repeat(32) }));
    const user = userEvent.setup();
    watchClipboard();
    renderCreate({ kind: "photos", ids: ["a"] });
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(await screen.findByRole("img", { name: "QR code of the link" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy link" }));
    expect(writeText).toHaveBeenLastCalledWith(`${window.location.origin}/s/${"Q".repeat(32)}`);
  });

  it("sends the chosen title, expiry preset and password, and warns about originals", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1));
    const user = userEvent.setup();
    const before = Date.now();
    renderCreate({ kind: "photos", ids: ["a"] });
    await user.type(screen.getByLabelText("Title (optional)"), "  For Sam ");
    await user.selectOptions(screen.getByLabelText("Link expires"), "1w");
    await user.type(screen.getByLabelText("Password (optional)"), "hunter22");
    await user.click(screen.getByLabelText(/Include originals/));
    expect(screen.getByText(/keep their GPS location/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create link" }));
    const body = create.mock.calls[0]![0];
    expect(body).toMatchObject({ asset_ids: ["a"], title: "For Sam", password: "hunter22", allow_download: true, include_originals: true });
    const days = (Date.parse(body.expires_at!) - before) / 86_400_000;
    expect(days).toBeGreaterThan(6.99);
    expect(days).toBeLessThan(7.01);
  });

  it("turning downloads off hides the originals choice and never sends it", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1));
    const user = userEvent.setup();
    renderCreate({ kind: "photos", ids: ["a"] });
    await user.click(screen.getByLabelText(/Include originals/));
    await user.click(screen.getByLabelText("Allow downloads"));
    expect(screen.queryByLabelText(/Include originals/)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(create.mock.calls[0]![0]).toMatchObject({ allow_download: false, include_originals: false });
  });

  it("shares an album by id and says that later additions appear", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1, { kind: "album" }));
    const user = userEvent.setup();
    renderCreate({ kind: "album", albumId: "alb", albumTitle: "Summer" });
    expect(screen.getByRole("heading", { name: "Share the album Summer" })).toBeInTheDocument();
    expect(screen.getByText(/add to the album later appear/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(create.mock.calls[0]![0]).toMatchObject({ album_id: "alb" });
    expect(create.mock.calls[0]![0]).not.toHaveProperty("asset_ids");
  });

  it("refuses more photos than a link holds, and shows server errors", async () => {
    const user = userEvent.setup();
    const { unmount } = renderCreate({ kind: "photos", ids: Array.from({ length: 1001 }, (_, i) => `a${i}`) });
    expect(screen.getByRole("alert")).toHaveTextContent(/up to 1000 photos/);
    expect(screen.getByRole("button", { name: "Create link" })).toBeDisabled();
    unmount();

    vi.spyOn(shareApi, "create").mockRejectedValue(new ApiError(400, "password must be 4 to 200 characters"));
    renderCreate({ kind: "photos", ids: ["a"] });
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("password must be 4 to 200 characters");
    expect(screen.getByRole("button", { name: "Create link" })).toBeEnabled();
  });

  it("falls back to asking for a manual copy when the clipboard is refused, and closes on Escape", async () => {
    vi.spyOn(shareApi, "create").mockResolvedValue(link(1));
    const onClose = vi.fn();
    const user = userEvent.setup();
    watchClipboard("denied");
    renderCreate({ kind: "photos", ids: ["a"] }, onClose);
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(await screen.findByText(/Copy the link above/)).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});

describe("EditShareDialog", () => {
  it("keeps the expiry and password unless told otherwise", async () => {
    const update = vi.spyOn(shareApi, "update").mockResolvedValue(link(1));
    const onSaved = vi.fn();
    const user = userEvent.setup();
    const s = link(1, { title: "Old", has_password: true, expires_at: "2099-01-01T00:00:00Z" });
    render(<EditShareDialog share={s} onClose={vi.fn()} onSaved={onSaved} />);
    expect(screen.getByLabelText("Link expires")).toHaveDisplayValue(/Keep: Expires in/);
    await user.clear(screen.getByLabelText("Title (optional)"));
    await user.type(screen.getByLabelText("Title (optional)"), "New");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(update).toHaveBeenCalledWith("share-1", { title: "New", allow_download: true, include_originals: false });
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
  });

  it("changes the expiry, sets a new password, or removes the password", async () => {
    const update = vi.spyOn(shareApi, "update").mockResolvedValue(link(1));
    const user = userEvent.setup();
    const s = link(1, { has_password: true, expires_at: "2099-01-01T00:00:00Z" });
    const { unmount } = render(<EditShareDialog share={s} onClose={vi.fn()} onSaved={vi.fn()} />);
    await user.selectOptions(screen.getByLabelText("Link expires"), "never");
    await user.type(screen.getByLabelText("New password"), "fresh pw");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(update.mock.calls[0]![1]).toMatchObject({ expires_at: null, password: "fresh pw" });
    unmount();

    render(<EditShareDialog share={s} onClose={vi.fn()} onSaved={vi.fn()} />);
    await user.click(screen.getByLabelText("Remove the password"));
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(update.mock.calls[1]![1]).toMatchObject({ password: null });
  });

  it("offers no password removal for a link without one, and shows errors", async () => {
    vi.spyOn(shareApi, "update").mockRejectedValue(new ApiError(404, "not found"));
    const user = userEvent.setup();
    render(<EditShareDialog share={link(1)} onClose={vi.fn()} onSaved={vi.fn()} />);
    expect(screen.queryByLabelText("Remove the password")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("not found");
  });
});
