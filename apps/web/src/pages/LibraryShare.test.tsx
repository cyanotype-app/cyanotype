import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { AuthProvider } from "../auth";
import { shareApi } from "../shareApi";
import { UploadProvider } from "../upload/UploadProvider";
import { detail, item } from "../test/fixtures";
import { fakeLibrary } from "../test/fakeLibrary";
import { link } from "../test/shareFixtures";
import { Library } from "./Library";

beforeEach(() => {
  vi.spyOn(api, "me").mockResolvedValue({ id: "u", email: "a@b.co" });
  fakeLibrary([item(1), item(2), item(3)]);
  vi.spyOn(api, "getAsset").mockImplementation(async (id) => detail(Number(id.split("-")[1])));
});
afterEach(() => vi.restoreAllMocks());

const renderLibrary = () =>
  render(
    <MemoryRouter>
      <AuthProvider>
        <UploadProvider deps={{ api, hash: async () => "0".repeat(64) }}>
          <Library />
        </UploadProvider>
      </AuthProvider>
    </MemoryRouter>,
  );

describe("Library: share", () => {
  it("shares the selected photos from the selection bar", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1));
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Select" }));
    const share = screen.getByRole("button", { name: "Share" });
    expect(share).toBeDisabled();
    await user.click(await screen.findByRole("button", { name: /Select photo-1/ }));
    await user.click(screen.getByRole("button", { name: /Select photo-2/ }));
    await user.click(screen.getByRole("button", { name: "Share" }));
    const dialog = screen.getByRole("dialog", { name: "Share 2 photos" });
    await user.click(within(dialog).getByRole("button", { name: "Create link" }));
    expect(create).toHaveBeenCalledWith(expect.objectContaining({ asset_ids: ["id-1", "id-2"] }));
    expect(await screen.findByLabelText("Share link")).toBeInTheDocument();
  });

  it("shares the open photo from the viewer", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1));
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: /Open photo-2/ }));
    await user.click(await screen.findByRole("button", { name: "Share" }));
    await user.click(screen.getByRole("button", { name: "Create link" }));
    expect(create).toHaveBeenCalledWith(expect.objectContaining({ asset_ids: ["id-2"] }));
    // Escape closes only the dialog, not the viewer behind it.
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: /Link created|Share/ })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "photo-2.jpg" })).toBeInTheDocument();
  });

  it("Select all shares the whole view by scope, minus the deselected photos", async () => {
    const create = vi.spyOn(shareApi, "create").mockResolvedValue(link(1));
    const user = userEvent.setup();
    renderLibrary();
    await user.click(await screen.findByRole("button", { name: "Select" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: /Select photo-2/ }));
    await user.click(screen.getByRole("button", { name: "Share" }));
    const dialog = screen.getByRole("dialog", { name: "Share 2 photos" });
    await user.click(within(dialog).getByRole("button", { name: "Create link" }));
    expect(create).toHaveBeenCalledWith(expect.objectContaining({ scope: "library", except: ["id-2"] }));
    expect(create.mock.calls[0]![0]).not.toHaveProperty("asset_ids");
  });
});
