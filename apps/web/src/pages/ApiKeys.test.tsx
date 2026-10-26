import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type ApiKey } from "../api";
import { ApiKeys } from "./ApiKeys";

const key = (n: number, over: Partial<ApiKey> = {}): ApiKey => ({
  id: `k${n}`,
  name: `key ${n}`,
  prefix: `cyk_ab12c${n}`,
  created_at: "2026-01-01T00:00:00Z",
  last_used_at: null,
  scope: "full",
  expires_at: null,
  expired: false,
  ...over,
});

afterEach(() => vi.restoreAllMocks());

describe("ApiKeys", () => {
  it("lists keys without secrets", async () => {
    vi.spyOn(api, "listApiKeys").mockResolvedValue({
      items: [key(1), key(2, { last_used_at: "2026-02-01T00:00:00Z" })],
    });
    render(<ApiKeys />);
    expect(await screen.findByText("key 1")).toBeInTheDocument();
    expect(screen.getByText("cyk_ab12c2…")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Revoke" })).toHaveLength(2);
  });

  it("shows how to install the command line from this server", async () => {
    vi.spyOn(api, "listApiKeys").mockResolvedValue({ items: [] });
    render(<ApiKeys />);
    await screen.findByText("No API keys yet.");
    expect(screen.getByText(`curl -fsSL ${window.location.origin}/cli/install.sh | sh`)).toBeInTheDocument();
    expect(screen.getByText(`irm ${window.location.origin}/cli/install.ps1 | iex`)).toBeInTheDocument();
  });

  it("creates a key, shows the secret once, and refreshes the list", async () => {
    const list = vi
      .spyOn(api, "listApiKeys")
      .mockResolvedValueOnce({ items: [] })
      .mockResolvedValue({ items: [key(1)] });
    const create = vi.spyOn(api, "createApiKey").mockResolvedValue({
      id: "k1",
      name: "laptop",
      prefix: "cyk_ab12c1",
      created_at: "2026-01-01T00:00:00Z",
      scope: "import",
      expires_at: null,
      key: "cyk_SECRET",
    });
    const user = userEvent.setup();
    render(<ApiKeys />);
    await screen.findByText("No API keys yet.");
    await user.type(screen.getByLabelText("Name"), "laptop");
    await user.click(screen.getByRole("button", { name: "Create key" }));
    expect(await screen.findByText("cyk_SECRET")).toBeInTheDocument();
    // The default is the smallest scope that works for the command line, with no expiry.
    expect(create).toHaveBeenCalledWith("laptop", "import", undefined);
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    await user.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByText("cyk_SECRET")).not.toBeInTheDocument();
  });

  it("revokes after confirmation", async () => {
    vi.spyOn(api, "listApiKeys")
      .mockResolvedValueOnce({ items: [key(1)] })
      .mockResolvedValue({ items: [] });
    const revoke = vi.spyOn(api, "revokeApiKey").mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<ApiKeys />);
    await user.click(await screen.findByRole("button", { name: "Revoke" }));
    expect(revoke).not.toHaveBeenCalled(); // the question comes first
    await user.click(within(screen.getByRole("alertdialog", { name: "Revoke “key 1”?" })).getByRole("button", { name: "Revoke" }));
    expect(revoke).toHaveBeenCalledWith("k1");
    expect(await screen.findByText("No API keys yet.")).toBeInTheDocument();
  });
});
